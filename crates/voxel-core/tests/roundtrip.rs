//! Integration tests for voxel-core.

use voxel_core::{ChunkBuffer, MaterialId, MaterialRegistry, ShapeId, StorageKind, Voxel};

#[test]
fn voxel_pack_unpack_full_field_sweep() {
    let materials = [MaterialId(0), MaterialId(1), MaterialId(0xFFFF)];
    let flag_sets = [0u8, 0x01, 0x80, 0xFF];

    for shape_disc in 0..=ShapeId::MAX_DISCRIMINANT {
        let shape = ShapeId::from_raw(shape_disc).unwrap();
        for &material in &materials {
            for &flags in &flag_sets {
                let v = Voxel { shape, material, flags };
                let bits = v.pack();
                assert_eq!(bits & 0xF800_0000, 0, "bits 27..=31 must be unused");
                let back = Voxel::unpack(bits).unwrap();
                assert_eq!(back, v);
            }
        }
    }
}

#[test]
fn chunk_buffer_stores_voxels() {
    let mut buf: ChunkBuffer<Voxel, 8> = ChunkBuffer::uniform(Voxel::EMPTY);
    let stone = Voxel::cube(MaterialId(1));
    buf.set(3, 4, 5, stone);
    assert_eq!(buf.kind(), StorageKind::Palette);
    assert_eq!(buf.get(3, 4, 5), stone);
    assert_eq!(buf.get(0, 0, 0), Voxel::EMPTY);
}

#[test]
fn full_chunk_iteration_count() {
    let buf: ChunkBuffer<u16, 8> = ChunkBuffer::uniform(0);
    let mut count = 0usize;
    buf.for_each(|_, _, _, _| count += 1);
    assert_eq!(count, ChunkBuffer::<u16, 8>::VOLUME);
}

#[test]
fn dense_promotion_preserves_pattern() {
    let mut buf: ChunkBuffer<u16, 4> = ChunkBuffer::uniform(0);
    // Write a recognizable pattern.
    for x in 0..4 {
        for y in 0..4 {
            for z in 0..4 {
                buf.set(x, y, z, (x * 16 + y * 4 + z) as u16);
            }
        }
    }
    buf.make_dense();
    for x in 0..4 {
        for y in 0..4 {
            for z in 0..4 {
                assert_eq!(buf.get(x, y, z), (x * 16 + y * 4 + z) as u16);
            }
        }
    }
}

#[test]
fn shape_discriminants_are_stable() {
    assert_eq!(ShapeId::Empty as u8, 0);
    assert_eq!(ShapeId::Cube as u8, 1);
    assert_eq!(ShapeId::SlabBottom as u8, 2);
    assert_eq!(ShapeId::SlabTop as u8, 3);
    assert_eq!(ShapeId::MAX_DISCRIMINANT, 3);
}

#[test]
fn material_registry_load_initial() {
    let reg = MaterialRegistry::load_initial();
    assert_eq!(reg.len(), 11);
    let grass = reg.resolve("grass_soil").expect("grass_soil resolves");
    assert_eq!(grass, MaterialId(6));
    assert_eq!(reg.get(grass).unwrap().display_name, "Grass Soil");
    assert!(reg.resolve("nonexistent").is_none());
}
