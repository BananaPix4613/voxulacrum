//! SQLite-based world persistence with delta compression.
//! Only player-modified chunks are stored. Base terrain is regenerated from seed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use rusqlite::{params, Connection, OpenFlags};

use super::chunk::{Chunk, VoxelEdit, DELTA_THRESHOLD};
use super::storage::{ChunkStorage, PopulatedChunk, PalettedBitArray};
use voxel_core::Voxel;

// -- Error type --------------------------------------------------------------

#[derive(Debug)]
pub enum PersistError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Corrupt(String),
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistError::Sqlite(e) => write!(f, "SQLite: {e}"),
            PersistError::Io(e) => write!(f, "IO: {e}"),
            PersistError::Corrupt(msg) => write!(f, "Corrupt: {msg}"),
        }
    }
}
impl std::error::Error for PersistError {}
impl From<rusqlite::Error> for PersistError { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<std::io::Error> for PersistError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }

// -- Format constants --------------------------------------------------------

const BLOB_VERSION: u8 = 4; // was 3 — chunk payloads now store packed u32 Voxels

/// Bumped whenever the on-disk voxel encoding changes. A stored value older than
/// this forces a one-shot save wipe on open (pre-release; saves are not migrated).
const VOXEL_FORMAT_VERSION: u64 = 1;

// The TAG_DELTA payload serializes only `index:u16 + packed voxel:u32`. Per-voxel
// moisture/flora state is intentionally NOT persisted yet: the moisture/flora
// simulation model is a later-phase concern. When it lands, the delta format will
// gain a per-edit flag byte selecting which optional fields follow each voxel.
// The previously-unused EDIT_FLAG_* bitflag scaffolding was removed so the source
// no longer implies a richer on-disk format than is actually written.

const TAG_DELTA: u8 = 0;
const TAG_FULL_UNIFORM: u8 = 1;
const TAG_FULL_POPULATED: u8 = 2;

// -- ChunkEdits --------------------------------------------------------------

/// What we store per modified chunk.
pub enum ChunkEdits {
    /// Sparse voxel changes (< DELTA_THRESHOLD edits).
    Delta(Vec<VoxelEdit>),
    /// Full chunk replacement (heavily modified or non-deterministic).
    Full(ChunkStorage),
}

// -- Serialization -----------------------------------------------------------

/// Serialize ChunkEdits -> raw bytes -> zstd compress.
pub fn serialize_chunk_edits_raw(edits: &ChunkEdits) -> Result<Vec<u8>, PersistError> {
    let mut raw = Vec::new();
    raw.push(BLOB_VERSION);

    match edits {
        ChunkEdits::Delta(edits) => {
            raw.push(TAG_DELTA);
            raw.extend_from_slice(&(edits.len() as u32).to_le_bytes());
            for edit in edits {
                raw.extend_from_slice(&edit.index.to_le_bytes());
                raw.extend_from_slice(&edit.voxel.pack().to_le_bytes());
            }
        }
        ChunkEdits::Full(storage) => match storage {
            ChunkStorage::Uniform { voxel } => {
                raw.push(TAG_FULL_UNIFORM);
                raw.extend_from_slice(&voxel.pack().to_le_bytes());
            }
            ChunkStorage::Populated(pop) => {
                raw.push(TAG_FULL_POPULATED);
                // material: palette-compressed
                pop.material_id.serialize_to_bytes(&mut raw);
            }
        },
    }

    Ok(raw)
}

/// Decompress zstd -> deserialize raw bytes -> ChunkEdits.
pub fn deserialize_chunk_edits_raw(raw: &[u8]) -> Result<ChunkEdits, PersistError> {
    if raw.len() < 2 {
        return Err(PersistError::Corrupt("blob too short".into()));
    }
    if raw[0] != BLOB_VERSION {
        return Err(PersistError::Corrupt(format!("unknown blob version {}", raw[0])));
    }

    match raw[1] {
        TAG_DELTA => {
            if raw.len() < 6 { return Err(PersistError::Corrupt("delta header short".into())); }
            let count = u32::from_le_bytes(raw[2..6].try_into().unwrap()) as usize;
            const EDIT_BYTES: usize = 6; // index:u16 + voxel:u32 (packed)
            if raw.len() < 6 + count * EDIT_BYTES {
                return Err(PersistError::Corrupt("delta data truncated".into()));
            }
            let mut edits = Vec::with_capacity(count);
            let mut off = 6;
            for _ in 0..count {
                let packed = u32::from_le_bytes(raw[off+2..off+6].try_into().unwrap());
                edits.push(VoxelEdit {
                    index: u16::from_le_bytes(raw[off..off+2].try_into().unwrap()),
                    voxel: Voxel::unpack(packed).unwrap_or(Voxel::EMPTY),
                    moisture: None,
                    flora_id: None,
                    flora_growth: None,
                });
                off += EDIT_BYTES;
            }
            Ok(ChunkEdits::Delta(edits))
        }
        TAG_FULL_UNIFORM => {
            if raw.len() < 6 { return Err(PersistError::Corrupt("uniform short".into())); }
            let packed = u32::from_le_bytes(raw[2..6].try_into().unwrap());
            Ok(ChunkEdits::Full(ChunkStorage::Uniform {
                voxel: Voxel::unpack(packed).unwrap_or(Voxel::EMPTY),
            }))
        }
        TAG_FULL_POPULATED => {
            let (material_id, _) = PalettedBitArray::deserialize_from_bytes(raw, 2)
                .ok_or_else(|| PersistError::Corrupt("material palette failed".into()))?;

            Ok(ChunkEdits::Full(ChunkStorage::Populated(Box::new(PopulatedChunk {
                material_id,
                lighting: None,
                simulation: None,
                flora: None,
            }))))
        }
        tag => Err(PersistError::Corrupt(format!("unknown tag {tag}"))),
    }
}

// -- WorldDatabase -----------------------------------------------------------

/// Wraps a rusqlite Connection in a Mutex so the struct is Send + Sync
/// (required for Bevy Resources and cross-thread sharing).
/// rusqlite::Connection is Send but NOT Sync due to internal RefCell.
pub struct WorldDatabase {
    conn: Mutex<Connection>,
    /// Raw zstd dictionary bytes for compression/decompression via bulk API.
    dict_bytes: Option<Vec<u8>>,
}

// Flags: 0 = plain zstd (legacy), 3 = zstd+dict
const FLAG_ZSTD_PLAIN: i64 = 0;
const FLAG_ZSTD_DICT: i64 = 3;

const DICT_SAMPLE_TARGET: usize = 200;
const DICT_SIZE: usize = 32 * 1024;

impl WorldDatabase {
    /// Open or create the database. Sets WAL mode and creates tables.
    pub fn open(path: &Path) -> Result<Self, PersistError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "cache_size", -32768)?; // 32 MB

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chunks (
                cx       INTEGER NOT NULL,
                cy       INTEGER NOT NULL,
                cz       INTEGER NOT NULL,
                flags    INTEGER NOT NULL DEFAULT 0,
                modified INTEGER NOT NULL DEFAULT 0,
                data     BLOB NOT NULL,
                PRIMARY KEY (cx, cy, cz)
            );"
        )?;

        let mut db = Self { conn: Mutex::new(conn), dict_bytes: None };
        // Try loading existing dictionary
        if let Ok(Some(bytes)) = db.get_meta_blob("zstd_dictionary") {
            log::info!("Loaded zstd dictionary ({} bytes)", bytes.len());
            db.dict_bytes = Some(bytes);
        }
        Ok(db)
    }

    /// Open a read-only connection (for worker threads).
    pub fn open_readonly(path: &Path, dict_bytes: Option<&[u8]>) -> Result<Self, PersistError> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        Ok(Self {
            conn: Mutex::new(conn),
            dict_bytes: dict_bytes.map(|b| b.to_vec()),
        })
    }

    pub fn try_train_dictionary(&mut self) -> Result<bool, PersistError> {
        if self.dict_bytes.is_some() { return Ok(false); }

        let count = self.chunk_count()?;
        if count < DICT_SAMPLE_TARGET as u64 { return Ok(false); }

        // Collect raw (decompressed) chunk blobs as training samples.
        // No dictionary exists yet, so all rows use plain zstd (FLAG_ZSTD_PLAIN).
        let samples = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT data FROM chunks LIMIT ?1")?;
            let mut rows = stmt.query(params![DICT_SAMPLE_TARGET as i64])?;
            let mut samples = Vec::with_capacity(DICT_SAMPLE_TARGET);
            while let Some(row) = rows.next()? {
                let blob: Vec<u8> = row.get(0)?;
                let raw = zstd::decode_all(blob.as_slice()).map_err(PersistError::Io)?;
                samples.push(raw);
            }
            samples
        };

        let trained = zstd::dict::from_samples(&samples, DICT_SIZE)
            .map_err(PersistError::Io)?;

        self.set_meta_blob("zstd_dictionary", &trained)?;
        log::info!("Trained zstd dictionary ({} bytes) from {} samples", trained.len(), samples.len());
        self.dict_bytes = Some(trained);
        Ok(true)
    }

    // -- Compression/Decompression --

    fn compress_blob(&self, raw: &[u8]) -> Result<(Vec<u8>, i64), PersistError> {
        if let Some(ref dict) = self.dict_bytes {
            let mut compressor = zstd::bulk::Compressor::with_dictionary(3, dict)
                .map_err(PersistError::Io)?;
            let compressed = compressor.compress(raw)
                .map_err(PersistError::Io)?;
            Ok((compressed, FLAG_ZSTD_DICT))
        } else {
            let compressed = zstd::encode_all(raw, 3)
                .map_err(PersistError::Io)?;
            Ok((compressed, FLAG_ZSTD_PLAIN))
        }
    }

    fn decompress_blob(&self, data: &[u8], flags: i64) -> Result<Vec<u8>, PersistError> {
        match flags {
            FLAG_ZSTD_PLAIN => zstd::decode_all(data).map_err(PersistError::Io),
            FLAG_ZSTD_DICT => {
                let dict = self.dict_bytes.as_ref()
                    .ok_or_else(|| PersistError::Corrupt("flags=3 but no dictionary".into()))?;
                let mut decompressor = zstd::bulk::Decompressor::with_dictionary(dict)
                    .map_err(PersistError::Io)?;
                // Upper bound: full populated chunk is ~65KB raw
                decompressor.decompress(data, 256 * 1024)
                    .map_err(PersistError::Io)
            }
            _ => Err(PersistError::Corrupt(format!("unknown flags {flags}"))),
        }
    }

    // -- Meta --

    pub fn get_meta_u64(&self, key: &str) -> Result<Option<u64>, PersistError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached("SELECT value FROM meta WHERE key = ?1")?;
        match stmt.query_row(params![key], |row| row.get::<_, Vec<u8>>(0)) {
            Ok(bytes) if bytes.len() == 8 => Ok(Some(u64::from_le_bytes(bytes.try_into().unwrap()))),
            Ok(_) => Err(PersistError::Corrupt(format!("meta '{key}' wrong size"))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_meta_u64(&self, key: &str, value: u64) -> Result<(), PersistError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value.to_le_bytes().as_slice()],
        )?;
        Ok(())
    }

    pub fn get_meta_blob(&self, key: &str) -> Result<Option<Vec<u8>>, PersistError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached("SELECT value FROM meta WHERE key = ?1")?;
        match stmt.query_row(params![key], |row| row.get::<_, Vec<u8>>(0)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_meta_blob(&self, key: &str, value: &[u8]) -> Result<(), PersistError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    // -- Chunk CRUD --

    /// Delete all saved chunk data (used when terrain params change).
    pub fn clear_all_chunks(&self) -> Result<usize, PersistError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute("DELETE FROM chunks", [])?;
        Ok(deleted)
    }

    pub fn load_chunk_edits(&self, pos: IVec3) -> Result<Option<ChunkEdits>, PersistError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT flags, data FROM chunks WHERE cx = ?1 AND cy = ?2 AND cz = ?3"
        )?;
        match stmt.query_row(params![pos.x, pos.y, pos.z], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        }) {
            Ok((flags, blob)) => {
                drop(stmt);
                drop(conn); // Release Mutex before calling decompress_blob (which locks conn)
                let raw = self.decompress_blob(&blob, flags)?;
                Ok(Some(deserialize_chunk_edits_raw(&raw)?))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_chunk(&self, pos: IVec3, edits: &ChunkEdits) -> Result<(), PersistError> {
        let raw = serialize_chunk_edits_raw(edits)?;
        let (blob, flags) = self.compress_blob(&raw)?;
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO chunks (cx,cy,cz,flags,modified,data) VALUES (?1,?2,?3,?4,?5,?6)",
            params![pos.x, pos.y, pos.z, flags, now, blob],
        )?;
        Ok(())
    }

    pub fn save_chunks_batch(&self, chunks: &[(IVec3, ChunkEdits)]) -> Result<usize, PersistError> {
        // Pre-compress all chunks outside the conn lock
        let mut prepared: Vec<(IVec3, Vec<u8>, i64)> = Vec::with_capacity(chunks.len());
        for (pos, edits) in chunks {
            let raw = serialize_chunk_edits_raw(edits)?;
            let (blob, flags) = self.compress_blob(&raw)?;
            prepared.push((*pos, blob, flags));
        }

        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut count = 0;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO chunks (cx,cy,cz,flags,modified,data) VALUES (?1,?2,?3,?4,?5,?6)"
            )?;
            for (pos, blob, flags) in &prepared {
                stmt.execute(params![pos.x, pos.y, pos.z, flags, now, blob])?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn chunk_count(&self) -> Result<u64, PersistError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_,i64>(0))? as u64)
    }
}

// -- Edit helpers ------------------------------------------------------------

/// Apply delta edits onto a base-generated ChunkStorage, returning a new one.
pub fn apply_edits_to_storage(base: &ChunkStorage, edits: &[VoxelEdit]) -> ChunkStorage {
    let mut storage = base.clone();
    for edit in edits {
        storage.set_voxel(edit.index as usize, edit.voxel);
        // Apply optional tier fields
        if edit.moisture.is_some() || edit.flora_id.is_some() || edit.flora_growth.is_some() {
            storage.ensure_populated();
            if let ChunkStorage::Populated(ref mut p) = storage {
                if let Some(moisture) = edit.moisture {
                    p.simulation_mut().moisture[edit.index as usize] = moisture;
                }
                if let Some(flora_id) = edit.flora_id {
                    p.flora_mut().flora_id.set(edit.index as usize, flora_id as u32);
                }
                if let Some(flora_growth) = edit.flora_growth {
                    p.flora_mut().flora_growth[edit.index as usize] = flora_growth;
                }
            }
        }
    }
    storage.try_collapse();
    storage
}

fn deduplicate_edits(edits: &[VoxelEdit]) -> Vec<VoxelEdit> {
    use std::collections::HashMap;
    // Map index -> position in result vec
    let mut seen: HashMap<u16, usize> = HashMap::with_capacity(edits.len());
    let mut result: Vec<VoxelEdit> = Vec::with_capacity(edits.len());
    for edit in edits {
        if let Some(&pos) = seen.get(&edit.index) {
            result[pos] = edit.clone();
        } else {
            seen.insert(edit.index, result.len());
            result.push(edit.clone());
        }
    }
    result
}

/// Build ChunkEdits from a Chunk's edit_list. Returns None if unmodified.
pub fn build_chunk_edits(chunk: &Chunk) -> Option<ChunkEdits> {
    if !chunk.persist_dirty { return None; }
    match &chunk.edit_list {
        None => {
            // Promoted or full replacement - save entire storage
            // Only if persist_dirty (which we checked above)
            Some(ChunkEdits::Full((*chunk.storage).clone()))
        }
        Some(list) if list.is_empty() => None,
        Some(list) => {
            let deduped = deduplicate_edits(list);
            if deduped.len() > DELTA_THRESHOLD {
                Some(ChunkEdits::Full((*chunk.storage).clone()))
            } else {
                Some(ChunkEdits::Delta(deduped))
            }
        }
    }
}

// -- WorldPersistence (Bevy Resource) ----------------------------------------

const AUTOSAVE_INTERVAL_SECS: f32 = 30.0;

#[derive(Resource)]
pub struct WorldPersistence {
    db: Option<WorldDatabase>,
    pub db_path: Option<PathBuf>,
    autosave_timer: f32,
    pub autosave_enabled: bool,
}

impl WorldPersistence {
    /// No persistence (fallback).
    pub fn disabled() -> Self {
        Self { db: None, db_path: None, autosave_timer: 0.0, autosave_enabled: false }
    }

    /// Open or create a world database.
    pub fn open(world_name: &str, seed: i32) -> Result<Self, PersistError> {
        let dir = crate::paths::asset_root().join("saves").join(world_name);
        let db_path = dir.join("world.vxdb");
        let mut db = WorldDatabase::open(&db_path)?;

        // Try training dictionary if enough chunks exist
        match db.try_train_dictionary() {
            Ok(true) => log::info!("Dictionary trained on first open"),
            Ok(false) => {} // Not enough chunks yet, or already trained
            Err(e) => log::warn!("Dictionary training failed: {e}"),
        }

        let seed_u64 = seed as u64;
        match db.get_meta_u64("seed")? {
            Some(stored) if stored != seed_u64 => {
                log::warn!("Seed mismatch: DB={stored}, params={seed_u64}. Updating DB.");
                db.set_meta_u64("seed", seed_u64)?;
            }
            None => { db.set_meta_u64("seed", seed_u64)?; }
            _ => {}
        }

        // One-shot save wipe when the on-disk voxel encoding is stale (pre-release;
        // saves are intentionally not migrated). Mirrors the regen cache-clear path.
        let stored_voxel_fmt = db.get_meta_u64("voxel_format_version")?.unwrap_or(0);
        if stored_voxel_fmt < VOXEL_FORMAT_VERSION {
            let cleared = db.clear_all_chunks()?;
            let cache_dir = crate::paths::asset_root().join("cache").join("meshes");
            let _ = crate::meshing::cache::clear_world_cache(&cache_dir);
            log::warn!(
                "Voxel format changed (saved v{stored_voxel_fmt} < v{VOXEL_FORMAT_VERSION}); \
                 wiped {cleared} saved chunks and the mesh cache. Pre-release: saves are not migrated."
            );
            db.set_meta_u64("voxel_format_version", VOXEL_FORMAT_VERSION)?;
        }

        // No `format_version` meta key is written: the per-chunk blob already
        // carries BLOB_VERSION and rejects mismatches on read (deserialize_chunk_
        // edits_raw), which semantic voxel-layout changes are gated by the
        // `voxel_format_version` wipe above. A separate write-only meta key
        // duplicated that and was never read, so it was removed. (Existing DBs may
        // still hold a stale `format_version` row; it is simply ignored.)

        log::info!("Persistence: {} ({} modified chunks)", db_path.display(), db.chunk_count()?);

        Ok(Self {
            db: Some(db),
            db_path: Some(db_path),
            autosave_timer: 0.0,
            autosave_enabled: true,
        })
    }

    pub fn is_active(&self) -> bool {
        self.db.is_some()
    }

    /// Clear all saved chunk data (called when terrain params change).
    pub fn clear_all_chunks(&self) -> Result<usize, PersistError> {
        if let Some(db) = &self.db {
            db.clear_all_chunks()
        } else {
            Ok(0)
        }
    }

    pub fn dictionary_bytes(&self) -> Option<Vec<u8>> {
        self.db.as_ref()?.get_meta_blob("zstd_dictionary").ok().flatten()
    }

    /// Save all dirty chunks in a batch transaction. Clears persist_dirty flags.
    pub fn save_dirty_chunks(&self, world: &mut super::World) -> Result<usize, PersistError> {
        let db = match &self.db { Some(db) => db, None => return Ok(0) };
        let mut batch: Vec<(IVec3, ChunkEdits)> = Vec::new();
        for chunk in world.chunks.values_mut() {
            if !chunk.persist_dirty { continue; }
            if let Some(edits) = build_chunk_edits(chunk) {
                batch.push((chunk.position, edits));
                chunk.persist_dirty = false;
            }
        }
        if batch.is_empty() { return Ok(0); }
        let count = db.save_chunks_batch(&batch)?;
        log::info!("Saved {count} modified chunks");
        Ok(count)
    }

    /// Tick auto-save timer. Saves when interval elapses.
    pub fn tick_autosave(&mut self, dt: f32, world: &mut super::World) -> bool {
        if !self.autosave_enabled || !self.is_active() { return false; }
        self.autosave_timer += dt;
        if self.autosave_timer < AUTOSAVE_INTERVAL_SECS { return false; }
        self.autosave_timer = 0.0;
        match self.save_dirty_chunks(world) {
            Ok(0) => false,
            Ok(_) => {
                // Attempt dictionary training if not yet done
                if let Some(ref mut db) = self.db {
                    let _ = db.try_train_dictionary();
                }
                true
            }
            Err(e) => { log::error!("Auto-save failed: {e}"); false }
        }
    }

    /// Save a single chunk before unload.
    pub fn save_chunk_on_unload(&self, chunk: &Chunk) -> Result<(), PersistError> {
        let db = match &self.db { Some(db) => db, None => return Ok(()) };
        if !chunk.persist_dirty { return Ok(()); }
        if let Some(edits) = build_chunk_edits(chunk) {
            db.save_chunk(chunk.position, &edits)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::chunk::CHUNK_VOLUME;
    use voxel_core::MaterialId;

    fn vox(mat: u16) -> Voxel {
        Voxel::cube(MaterialId(mat))
    }

    /// Delta payloads round-trip: index + packed voxel survive exactly, and the
    /// deferred moisture/flora fields decode to None (they are not serialized).
    #[test]
    fn roundtrip_delta() {
        let edits = vec![
            VoxelEdit { index: 0,    voxel: vox(1),       moisture: None, flora_id: None, flora_growth: None },
            VoxelEdit { index: 17,   voxel: vox(3),       moisture: None, flora_id: None, flora_growth: None },
            VoxelEdit { index: 4095, voxel: Voxel::EMPTY, moisture: None, flora_id: None, flora_growth: None },
        ];
        let raw = serialize_chunk_edits_raw(&ChunkEdits::Delta(edits.clone())).unwrap();
        match deserialize_chunk_edits_raw(&raw).unwrap() {
            ChunkEdits::Delta(back) => {
                assert_eq!(back.len(), edits.len());
                for (a, b) in edits.iter().zip(back.iter()) {
                    assert_eq!(a.index, b.index);
                    assert_eq!(a.voxel, b.voxel);
                    assert!(b.moisture.is_none() && b.flora_id.is_none() && b.flora_growth.is_none());
                }
            }
            other => panic!("expected Delta, got a different variant: {}", variant_name(&other)),
        }
    }

    // Full Uniform payloads round-trip the single voxel.
    #[test]
    fn roundtrip_full_uniform() {
        let voxel = vox(5);
        let raw = serialize_chunk_edits_raw(&ChunkEdits::Full(ChunkStorage::Uniform { voxel })).unwrap();
        match deserialize_chunk_edits_raw(&raw).unwrap() {
            ChunkEdits::Full(ChunkStorage::Uniform { voxel: back }) => assert_eq!(voxel, back),
            other => panic!("expected Full Uniform, got: {}", variant_name(&other)),
        }
    }

    /// Full Populated payloads round-trip every voxel's packed material id.
    #[test]
    fn roundtrip_full_populated() {
        let mut material_id = PalettedBitArray::new(Voxel::EMPTY.pack());
        material_id.set(0, vox(2).pack());
        material_id.set(100, vox(6).pack());
        material_id.set(CHUNK_VOLUME - 1, vox(4).pack());

        let storage = ChunkStorage::Populated(Box::new(PopulatedChunk {
            material_id: material_id.clone(),
            lighting: None,
            simulation: None,
            flora: None,
        }));
        let raw = serialize_chunk_edits_raw(&ChunkEdits::Full(storage)).unwrap();
        match deserialize_chunk_edits_raw(&raw).unwrap() {
            ChunkEdits::Full(ChunkStorage::Populated(pop)) => {
                for i in [0usize, 1, 100, CHUNK_VOLUME - 1] {
                    assert_eq!(pop.material_id.get(i), material_id.get(i), "voxel {i} mismatch");
                }
            }
            other => panic!("expected Full Populated, got: {}", variant_name(&other)),
        }
    }

    /// A blob whose version byte does not match BLOB_VERSION is rejected, never
    /// silently misinterpreted.
    #[test]
    fn rejects_unknown_blob_version() {
        let mut raw = serialize_chunk_edits_raw(
            &ChunkEdits::Full(ChunkStorage::Uniform { voxel: vox(1) }),
        ).unwrap();
        raw[0] = 0xFF; // clobber BLOB_VERSION
        assert!(deserialize_chunk_edits_raw(&raw).is_err());
    }

    fn variant_name(edits: &ChunkEdits) -> &'static str {
        match edits {
            ChunkEdits::Delta(_) => "Delta",
            ChunkEdits::Full(ChunkStorage::Uniform { .. }) => "Full Uniform",
            ChunkEdits::Full(ChunkStorage::Populated(_)) => "Full Populated",
        }
    }
}
