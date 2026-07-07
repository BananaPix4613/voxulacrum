//! SQLite-based world persistence with delta compression.
//! Only player-modified chunks are stored. Base terrain is regenerated from seed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use rusqlite::{params, Connection, OpenFlags};

use smallvec::SmallVec;

use super::chunk::{LoadedChunk, DELTA_THRESHOLD};
use super::layers::{
    DecalEntry, DetailLayerId, DetailTexel, FluidCell, FluidId, PrefabId,
    ScatterFlags, ScatterInstance, StableInstanceId,
};
use super::overrides::ChunkOverrides;
use super::storage::{ChunkStorage, PopulatedChunk, PalettedBitArray};
use super::tags::{BiomeId, ChunkTags, LibraryGraphId, ZoneId};
use voxel_core::{FaceAxis, LocalPos, Voxel};

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

const BLOB_VERSION: u8 = 6; // v6 - scatter instances carry stable_id

/// Bumped whenever the on-disk voxel/override encoding changes. A stored value
/// older than this forces a one-shot save wipe on open (pre-release; saves are
/// not migrated). v3: scatter instance gained a stable_id field.
const VOXEL_FORMAT_VERSION: u64 = 3;

// A v6 blob is `[BLOB_VERSION][variant_tag]<variant payload><tags section>`.
// The variant payload is one of:
//   TAG_DELTA          -> every ChunkOverrides field, each a u32-count-prefixed
//                         section; map-backed sections are key-sorted so an
//                         identical override set always produces identical bytes.
//   TAG_FULL_UNIFORM   -> the single packed u32 voxel.
//   TAG_FULL_POPULATED -> the palette-compressed material array (self-delimiting).
// The tags section (zone + biomes + library refs) is appended after the payload
// in every variant, so each chunk round-trips its identity tags. In Phase 3 only
// `voxel_diffs` and the trivial tags are non-empty; the remaining override layers
// round-trip but have no runtime consumer until their systems land.

const TAG_DELTA: u8 = 0;
const TAG_FULL_UNIFORM: u8 = 1;
const TAG_FULL_POPULATED: u8 = 2;

// -- ChunkEdits --------------------------------------------------------------

/// What we store per modified chunk.
pub enum ChunkEdits {
    /// Sparse voxel overrides (< DELTA_THRESHOLD voxel diffs).
    Delta(ChunkOverrides),
    /// Full chunk replacement (heavily modified or non-deterministic).
    Full(ChunkStorage),
}

/// The full per-chunk persisted unit: a chunk's edits plus its identity tags.
/// This is the value the blob serializer round-trips.
pub struct ChunkRecord {
    /// Voxel edits (sparse delta or full snapshot).
    pub edits: ChunkEdits,
    /// Zone / biome / library identity (design doc §16 targeted invalidation).
    pub tags: ChunkTags,
}

// -- Serialization -----------------------------------------------------------

// Little-endian write helpers (kept terse; the matching reads live on `Reader`).
fn w_u16(buf: &mut Vec<u8>, v: u16) { buf.extend_from_slice(&v.to_le_bytes()); }
fn w_u32(buf: &mut Vec<u8>, v: u32) { buf.extend_from_slice(&v.to_le_bytes()); }
fn w_u64(buf: &mut Vec<u8>, v: u64) { buf.extend_from_slice(&v.to_le_bytes()); }

/// A bounds-checked cursor over a raw blob. Every read advances `pos` and errors
/// (rather than panicking) when the buffer is too short, so a truncated or
/// corrupt blob is rejected cleanly.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], PersistError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| PersistError::Corrupt("length overflow".into()))?;
        if end > self.buf.len() {
            return Err(PersistError::Corrupt("unexpected end of blob".into()));
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, PersistError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, PersistError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, PersistError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, PersistError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Stable on-disk encoding for a face axis (decoupled from the enum's in-memory
/// discriminants, so reordering the enum can't silently corrupt saves).
fn face_to_u8(face: FaceAxis) -> u8 {
    match face {
        FaceAxis::PosX => 0,
        FaceAxis::NegX => 1,
        FaceAxis::PosY => 2,
        FaceAxis::NegY => 3,
        FaceAxis::PosZ => 4,
        FaceAxis::NegZ => 5,
    }
}

fn face_from_u8(v: u8) -> Result<FaceAxis, PersistError> {
    Ok(match v {
        0 => FaceAxis::PosX,
        1 => FaceAxis::NegX,
        2 => FaceAxis::PosY,
        3 => FaceAxis::NegY,
        4 => FaceAxis::PosZ,
        5 => FaceAxis::NegZ,
        other => return Err(PersistError::Corrupt(format!("bad face axis {other}"))),
    })
}

// A scatter instance encodes to a fixed 20 bytes:
//   anchor index u16 | sub_offset 3xi8 | rotation_y u8 | scale_variant u8
//   | prefab_id u32 | flags u8 | stable_id u64
fn write_scatter_instance(buf: &mut Vec<u8>, inst: &ScatterInstance) {
    w_u16(buf, inst.anchor.to_index() as u16);
    buf.push(inst.sub_offset[0] as u8);
    buf.push(inst.sub_offset[1] as u8);
    buf.push(inst.sub_offset[2] as u8);
    buf.push(inst.rotation_y);
    buf.push(inst.scale_variant);
    w_u32(buf, inst.prefab_id.0);
    buf.push(inst.flags.0);
    w_u64(buf, inst.stable_id.0);
}

fn read_scatter_instance(r: &mut Reader) -> Result<ScatterInstance, PersistError> {
    let anchor = LocalPos::from_index(r.u16()? as usize);
    let sub_offset = [r.u8()? as i8, r.u8()? as i8, r.u8()? as i8];
    let rotation_y = r.u8()?;
    let scale_variant = r.u8()?;
    let prefab_id = PrefabId(r.u32()?);
    let flags = ScatterFlags(r.u8()?);
    let stable_id = StableInstanceId(r.u64()?);
    Ok(ScatterInstance { anchor, sub_offset, rotation_y, scale_variant, prefab_id, flags, stable_id })
}

/// Encode all seven `ChunkOverrides` fields. Each is a `u32` count followed by
/// its entries; map-backed sections are sorted by key so identical override sets
/// serialize to identical bytes regardless of `HashMap` iteration order.
fn write_overrides(buf: &mut Vec<u8>, ovr: &ChunkOverrides) {
    // 1. voxel_diffs: index u16 + packed voxel u32
    let mut voxel_diffs: Vec<(u16, u32)> = ovr
        .voxel_diffs
        .iter()
        .map(|(pos, v)| (pos.to_index() as u16, v.pack()))
        .collect();
    voxel_diffs.sort_unstable_by_key(|&(index, _)| index);
    w_u32(buf, voxel_diffs.len() as u32);
    for (index, packed) in voxel_diffs {
        w_u16(buf, index);
        w_u32(buf, packed);
    }

    // 2. voxel_removed: index u16
    let mut voxel_removed: Vec<u16> =
        ovr.voxel_removed.iter().map(|pos| pos.to_index() as u16).collect();
    voxel_removed.sort_unstable();
    w_u32(buf, voxel_removed.len() as u32);
    for index in voxel_removed {
        w_u16(buf, index);
    }

    // 3. scatter_removed: stable instance id u64
    let mut scatter_removed: Vec<u64> =
        ovr.scatter_removed.iter().map(|id| id.0).collect();
    scatter_removed.sort_unstable();
    w_u32(buf, scatter_removed.len() as u32);
    for id in scatter_removed {
        w_u64(buf, id);
    }

    // 4. scatter_added: Vec order is meaningful, so it is preserved verbatim.
    w_u32(buf, ovr.scatter_added.len() as u32);
    for inst in &ovr.scatter_added {
        write_scatter_instance(buf, inst);
    }

    // 5. detail_diffs: layer u16 + index u16 + texel(species,density,tint,flags)
    let mut detail: Vec<(u16, u16, DetailTexel)> = ovr
        .detail_diffs
        .iter()
        .map(|((layer, pos), texel)| (layer.0, pos.to_index() as u16, *texel))
        .collect();
    detail.sort_unstable_by_key(|&(layer, index, _)| (layer, index));
    w_u32(buf, detail.len() as u32);
    for (layer, index, texel) in detail {
        w_u16(buf, layer);
        w_u16(buf, index);
        buf.push(texel.species);
        buf.push(texel.density);
        buf.push(texel.tint);
        buf.push(texel.flags);
    }

    // 6. fluid_diffs: index u16 + fluid_id u16 + mass u16 + flags u8
    let mut fluids: Vec<(u16, FluidCell)> = ovr
        .fluid_diffs
        .iter()
        .map(|(pos, cell)| (pos.to_index() as u16, *cell))
        .collect();
    fluids.sort_unstable_by_key(|&(index, _)| index);
    w_u32(buf, fluids.len() as u32);
    for (index, cell) in fluids {
        w_u16(buf, index);
        w_u16(buf, cell.fluid_id.0);
        w_u16(buf, cell.mass);
        buf.push(cell.flags);
    }

    // 7. decal_diffs: index u16 + face u8 + decal_id u16 + flags u8
    let mut decals: Vec<(u16, u8, DecalEntry)> = ovr
        .decal_diffs
        .iter()
        .map(|((pos, face), entry)| (pos.to_index() as u16, face_to_u8(*face), *entry))
        .collect();
    decals.sort_unstable_by_key(|&(index, face, _)| (index, face));
    w_u32(buf, decals.len() as u32);
    for (index, face, entry) in decals {
        w_u16(buf, index);
        buf.push(face);
        w_u16(buf, entry.decal_id);
        buf.push(entry.flags);
    }
}

fn read_overrides(r: &mut Reader) -> Result<ChunkOverrides, PersistError> {
    let mut ovr = ChunkOverrides::default();

    // 1. voxel_diffs
    let n = r.u32()? as usize;
    for _ in 0..n {
        let index = r.u16()? as usize;
        let packed = r.u32()?;
        ovr.voxel_diffs.insert(
            LocalPos::from_index(index),
            Voxel::unpack(packed).unwrap_or(Voxel::EMPTY),
        );
    }

    // 2. voxel_removed
    let n = r.u32()? as usize;
    for _ in 0..n {
        ovr.voxel_removed.insert(LocalPos::from_index(r.u16()? as usize));
    }

    // 3. scatter_removed
    let n = r.u32()? as usize;
    for _ in 0..n {
        ovr.scatter_removed.insert(StableInstanceId(r.u64()?));
    }

    // 4. scatter_added
    let n = r.u32()? as usize;
    for _ in 0..n {
        ovr.scatter_added.push(read_scatter_instance(r)?);
    }

    // 5. detail_diffs
    let n = r.u32()? as usize;
    for _ in 0..n {
        let layer = DetailLayerId(r.u16()?);
        let pos = LocalPos::from_index(r.u16()? as usize);
        let texel = DetailTexel {
            species: r.u8()?,
            density: r.u8()?,
            tint: r.u8()?,
            flags: r.u8()?,
        };
        ovr.detail_diffs.insert((layer, pos), texel);
    }

    // 6. fluid_diffs
    let n = r.u32()? as usize;
    for _ in 0..n {
        let pos = LocalPos::from_index(r.u16()? as usize);
        let cell = FluidCell {
            fluid_id: FluidId(r.u16()?),
            mass: r.u16()?,
            flags: r.u8()?,
        };
        ovr.fluid_diffs.insert(pos, cell);
    }

    // 7. decal_diffs
    let n = r.u32()? as usize;
    for _ in 0..n {
        let pos = LocalPos::from_index(r.u16()? as usize);
        let face = face_from_u8(r.u8()?)?;
        let entry = DecalEntry { decal_id: r.u16()?, flags: r.u8()? };
        ovr.decal_diffs.insert((pos, face), entry);
    }

    Ok(ovr)
}

fn write_tags(buf: &mut Vec<u8>, tags: &ChunkTags) {
    w_u16(buf, tags.zone.0);
    w_u32(buf, tags.biomes.len() as u32);
    for biome in &tags.biomes {
        w_u16(buf, biome.0);
    }
    w_u32(buf, tags.library_refs.len() as u32);
    for r in &tags.library_refs {
        w_u32(buf, r.0);
    }
}

fn read_tags(r: &mut Reader) -> Result<ChunkTags, PersistError> {
    let zone = ZoneId(r.u16()?);
    let mut biomes: SmallVec<[BiomeId; 4]> = SmallVec::new();
    let n = r.u32()? as usize;
    for _ in 0..n {
        biomes.push(BiomeId(r.u16()?));
    }
    let mut library_refs: SmallVec<[LibraryGraphId; 8]> = SmallVec::new();
    let n = r.u32()? as usize;
    for _ in 0..n {
        library_refs.push(LibraryGraphId(r.u32()?));
    }
    Ok(ChunkTags { zone, biomes, library_refs })
}

/// Serialize a ChunkRecord -> raw bytes (caller zstd-compress).
pub fn serialize_chunk_record_raw(record: &ChunkRecord) -> Result<Vec<u8>, PersistError> {
    let mut raw = Vec::new();
    raw.push(BLOB_VERSION);

    match &record.edits {
        ChunkEdits::Delta(overrides) => {
            raw.push(TAG_DELTA);
            write_overrides(&mut raw, overrides);
        }
        ChunkEdits::Full(ChunkStorage::Uniform { voxel }) => {
            raw.push(TAG_FULL_UNIFORM);
            w_u32(&mut raw, voxel.pack());
        }
        ChunkEdits::Full(ChunkStorage::Populated(pop)) => {
            raw.push(TAG_FULL_POPULATED);
            // material: palette-compressed (self-delimiting)
            pop.material_id.serialize_to_bytes(&mut raw);
        }
    }

    write_tags(&mut raw, &record.tags);
    Ok(raw)
}

/// Deserialize raw bytes (post zstd-decompress) -> ChunkRecord.
pub fn deserialize_chunk_record_raw(raw: &[u8]) -> Result<ChunkRecord, PersistError> {
    let mut r = Reader::new(raw);
    let version = r.u8()?;
    if version != BLOB_VERSION {
        return Err(PersistError::Corrupt(format!("unknown blob version {version}")));
    }

    let variant_tag = r.u8()?;
    let edits = match variant_tag {
        TAG_DELTA => ChunkEdits::Delta(read_overrides(&mut r)?),
        TAG_FULL_UNIFORM => {
            let packed = r.u32()?;
            ChunkEdits::Full(ChunkStorage::Uniform {
                voxel: Voxel::unpack(packed).unwrap_or(Voxel::EMPTY),
            })
        }
        TAG_FULL_POPULATED => {
            // Bridge to the palette array's self-delimiting decoder, then resync
            // the cursor to the absolute offset it reports.
            let (material_id, new_pos) =
                PalettedBitArray::deserialize_from_bytes(r.buf, r.pos)
                    .ok_or_else(|| PersistError::Corrupt("material palette failed".into()))?;
            r.pos = new_pos;
            ChunkEdits::Full(ChunkStorage::Populated(Box::new(PopulatedChunk { material_id })))
        }
        other => return Err(PersistError::Corrupt(format!("unknown tag {other}"))),
    };

    let tags = read_tags(&mut r)?;
    Ok(ChunkRecord { edits, tags })
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

    pub fn load_chunk_record(&self, pos: IVec3) -> Result<Option<ChunkRecord>, PersistError> {
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
                Ok(Some(deserialize_chunk_record_raw(&raw)?))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_chunk(&self, pos: IVec3, edits: &ChunkRecord) -> Result<(), PersistError> {
        let raw = serialize_chunk_record_raw(edits)?;
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

    pub fn save_chunks_batch(&self, chunks: &[(IVec3, ChunkRecord)]) -> Result<usize, PersistError> {
        // Pre-compress all chunks outside the conn lock
        let mut prepared: Vec<(IVec3, Vec<u8>, i64)> = Vec::with_capacity(chunks.len());
        for (pos, edits) in chunks {
            let raw = serialize_chunk_record_raw(edits)?;
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

/// Apply a chunk's voxel overrides onto a base-generated ChunkStorage,
/// returning a new one. Only `voxel_diffs` is applied in Phase 3.
pub fn apply_overrides_to_storage(base: &ChunkStorage, overrides: &ChunkOverrides) -> ChunkStorage {
    let mut storage = base.clone();
    for (pos, voxel) in &overrides.voxel_diffs {
        storage.set_voxel(pos.to_index(), *voxel);
    }
    storage.try_collapse();
    storage
}

/// Build ChunkEdits from a Chunk's overrides. Returns None if unmodified.
pub fn build_chunk_edits(chunk: &LoadedChunk) -> Option<ChunkEdits> {
    if !chunk.persist_dirty { return None; }
    match &chunk.data.overrides {
        None => {
            // Promoted or full replacement - save entire storage
            // (persist_dirty already checked above).
            Some(ChunkEdits::Full((*chunk.data.voxels).clone()))
        }
        Some(ovr) if ovr.is_empty() => None,
        Some(ovr) => {
            if ovr.voxel_override_count() > DELTA_THRESHOLD {
                Some(ChunkEdits::Full((*chunk.data.voxels).clone()))
            } else {
                Some(ChunkEdits::Delta(ovr.clone()))
            }
        }
    }
}

/// Build the full persisted record (edits + a snapshot of the chunk's tags).
/// Returns None when the chunk has no edits to save.
pub fn build_chunk_record(chunk: &LoadedChunk) -> Option<ChunkRecord> {
    let edits = build_chunk_edits(chunk)?;
    Some(ChunkRecord { edits, tags: chunk.data.tags.clone() })
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
        let mut batch: Vec<(IVec3, ChunkRecord)> = Vec::new();
        for chunk in world.chunks.values_mut() {
            if !chunk.persist_dirty { continue; }
            if let Some(record) = build_chunk_record(chunk) {
                batch.push((chunk.data.coord.into(), record));
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
    pub fn save_chunk_on_unload(&self, chunk: &LoadedChunk) -> Result<(), PersistError> {
        let db = match &self.db { Some(db) => db, None => return Ok(()) };
        if !chunk.persist_dirty { return Ok(()); }
        if let Some(record) = build_chunk_record(chunk) {
            db.save_chunk(chunk.data.coord.into(), &record)?;
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

    /// Overrides exercising every field, with at least two entries in the sortable
    /// sections so key-ordering is actually tested.
    fn sample_overrides() -> ChunkOverrides {
        let mut ovr = ChunkOverrides::default();

        ovr.voxel_diffs.insert(LocalPos::from_index(0), vox(1));
        ovr.voxel_diffs.insert(LocalPos::from_index(50), Voxel::EMPTY);

        ovr.voxel_removed.insert(LocalPos::from_index(7));
        ovr.voxel_removed.insert(LocalPos::from_index(9000));

        ovr.scatter_removed.insert(StableInstanceId(0xDEAD_BEEF));
        ovr.scatter_removed.insert(StableInstanceId(42));

        ovr.scatter_added.push(ScatterInstance {
            anchor: LocalPos::from_index(123),
            sub_offset: [-1, 2, -3],
            rotation_y: 200,
            scale_variant: 4,
            prefab_id: PrefabId(77),
            flags: ScatterFlags(ScatterFlags::PLAYER_PLACED),
            stable_id: StableInstanceId(0x0123_4567_89AB_CDEF),
        });
        ovr.scatter_added.push(ScatterInstance {
            anchor: LocalPos::from_index(456),
            sub_offset: [0, 0, 0],
            rotation_y: 0,
            scale_variant: 0,
            prefab_id: PrefabId(1),
            flags: ScatterFlags::default(),
            stable_id: StableInstanceId::default(),
        });

        ovr.detail_diffs.insert(
            (DetailLayerId(2), LocalPos::from_index(33)),
            DetailTexel { species: 5, density: 200, tint: 3, flags: 1 },
        );
        ovr.detail_diffs.insert(
            (DetailLayerId(0), LocalPos::from_index(33)),
            DetailTexel { species: 9, density: 1, tint: 0, flags: 0 },
        );

        ovr.fluid_diffs.insert(
            LocalPos::from_index(64),
            FluidCell { fluid_id: FluidId(1), mass: 65535, flags: FluidCell::FLAG_SOURCE },
        );

        ovr.decal_diffs.insert(
            (LocalPos::from_index(88), FaceAxis::PosY),
            DecalEntry { decal_id: 9, flags: 2 },
        );
        ovr.decal_diffs.insert(
            (LocalPos::from_index(88), FaceAxis::NegX),
            DecalEntry { decal_id: 4, flags: 0 },
        );

        ovr
    }

    fn sample_tags() -> ChunkTags {
        let mut tags = ChunkTags::single_biome(ZoneId(3), BiomeId(7));
        tags.biomes.push(BiomeId(8));
        tags.library_refs.push(LibraryGraphId(100));
        tags.library_refs.push(LibraryGraphId(200));
        tags
    }

    /// A Delta record round-trips every override field and its tags exactly.
    #[test]
    fn roundtrip_delta_all_fields() {
        let overrides = sample_overrides();
        let tags = sample_tags();
        let record = ChunkRecord {
            edits: ChunkEdits::Delta(overrides.clone()),
            tags: tags.clone(),
        };
        let raw = serialize_chunk_record_raw(&record).unwrap();
        let back = deserialize_chunk_record_raw(&raw).unwrap();
        assert_eq!(back.tags, tags);
        match back.edits {
            ChunkEdits::Delta(b) => assert_eq!(b, overrides),
            other => panic!("expected Delta, got: {}", variant_name(&other)),
        }
    }

    /// Full Uniform records round-trip the single voxel and the tags.
    #[test]
    fn roundtrip_full_uniform() {
        let voxel = vox(5);
        let tags = sample_tags();
        let record = ChunkRecord {
            edits: ChunkEdits::Full(ChunkStorage::Uniform { voxel }),
            tags: tags.clone(),
        };
        let raw = serialize_chunk_record_raw(&record).unwrap();
        let back = deserialize_chunk_record_raw(&raw).unwrap();
        assert_eq!(back.tags, tags);
        match back.edits {
            ChunkEdits::Full(ChunkStorage::Uniform { voxel: b }) => assert_eq!(voxel, b),
            other => panic!("expected Full Uniform, got: {}", variant_name(&other)),
        }
    }

    /// Full Populated records round-trip every voxel's packed material id + tags.
    #[test]
    fn roundtrip_full_populated() {
        let mut material_id = PalettedBitArray::new(Voxel::EMPTY.pack());
        material_id.set(0, vox(2).pack());
        material_id.set(100, vox(6).pack());
        material_id.set(CHUNK_VOLUME - 1, vox(4).pack());

        let tags = sample_tags();
        let record = ChunkRecord {
            edits: ChunkEdits::Full(ChunkStorage::Populated(Box::new(PopulatedChunk {
                material_id: material_id.clone(),
            }))),
            tags: tags.clone(),
        };
        let raw = serialize_chunk_record_raw(&record).unwrap();
        let back = deserialize_chunk_record_raw(&raw).unwrap();
        assert_eq!(back.tags, tags);
        match back.edits {
            ChunkEdits::Full(ChunkStorage::Populated(pop)) => {
                for i in [0usize, 1, 100, CHUNK_VOLUME - 1] {
                    assert_eq!(pop.material_id.get(i), material_id.get(i), "voxel {i} mismatch");
                }
            }
            other => panic!("expected Full Populated, got: {}", variant_name(&other)),
        }
    }

    /// An empty Delta with default tags is the minimal blob: version + tag, seven
    /// zero-count override sections, then zone u16 + two zero-count tag sections.
    #[test]
    fn roundtrip_empty_is_compact() {
        let record = ChunkRecord {
            edits: ChunkEdits::Delta(ChunkOverrides::default()),
            tags: ChunkTags::default(),
        };
        let raw = serialize_chunk_record_raw(&record).unwrap();
        assert_eq!(raw.len(), 2 + 7 * 4 + 2 + 4 + 4, "empty v5 blob should be 40 bytes");

        let back = deserialize_chunk_record_raw(&raw).unwrap();
        assert_eq!(back.tags, ChunkTags::default());
        match back.edits {
            ChunkEdits::Delta(b) => assert!(b.is_empty()),
            other => panic!("expected Delta, got: {}", variant_name(&other)),
        }
    }

    /// Identical override sets built in different insertion orders must serialize
    /// to byte-identical blobs (HashMap iteration order is not stable).
    #[test]
    fn serialization_is_order_independent() {
        let mut a = ChunkOverrides::default();
        a.voxel_diffs.insert(LocalPos::from_index(5), vox(1));
        a.voxel_diffs.insert(LocalPos::from_index(1), vox(2));
        a.voxel_diffs.insert(LocalPos::from_index(9), vox(3));

        let mut b = ChunkOverrides::default();
        b.voxel_diffs.insert(LocalPos::from_index(9), vox(3));
        b.voxel_diffs.insert(LocalPos::from_index(1), vox(2));
        b.voxel_diffs.insert(LocalPos::from_index(5), vox(1));

        let tags = ChunkTags::default();
        let ra = serialize_chunk_record_raw(&ChunkRecord {
            edits: ChunkEdits::Delta(a),
            tags: tags.clone(),
        })
            .unwrap();
        let rb = serialize_chunk_record_raw(&ChunkRecord {
            edits: ChunkEdits::Delta(b),
            tags,
        })
            .unwrap();
        assert_eq!(ra, rb);
    }

    /// A blob whose version byte does not match BLOB_VERSION is rejected, never
    /// silently misinterpreted.
    #[test]
    fn rejects_unknown_blob_version() {
        let mut raw = serialize_chunk_record_raw(&ChunkRecord {
            edits: ChunkEdits::Full(ChunkStorage::Uniform { voxel: vox(1) }),
            tags: ChunkTags::default(),
        })
            .unwrap();
        raw[0] = 0xFF; // clobber BLOB_VERSION
        assert!(deserialize_chunk_record_raw(&raw).is_err());
    }

    fn variant_name(edits: &ChunkEdits) -> &'static str {
        match edits {
            ChunkEdits::Delta(_) => "Delta",
            ChunkEdits::Full(ChunkStorage::Uniform { .. }) => "Full Uniform",
            ChunkEdits::Full(ChunkStorage::Populated(_)) => "Full Populated",
        }
    }
}
