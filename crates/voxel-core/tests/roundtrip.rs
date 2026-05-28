//! Integration tests for voxel-core.

use voxel_core::{ChunkBuffer, MaterialId, Rotation, ShapeId, StorageKind, Voxel};

#[test]
fn voxel_pack_unpack_full_field_sweep() {
    // Sweep through every shape × every rotation × a few materials × a few flags.
    let materials = [MaterialId(0), MaterialId(1), MaterialId(0xFFFF)];
    let flag_sets = [0u8, 0x01, 0x80, 0xFF];
    
    for shape_disc in 0..=ShapeId::MAX_DISCRIMINANT {
        let shape = ShapeId::from_raw(shape_disc).unwrap();
        for rot_raw in 0..=3u8 {
            let rotation = Rotation::from_raw(rot_raw);
            for &material in &materials {
                for &flags in &flag_sets {
                    let v = Voxel { shape, rotation, material, flags };
                    let bits = v.pack();
                    assert_eq!(bits & 0x8000_0000, 0, "high bit must be unused");
                    let back = Voxel::unpack(bits).unwrap();
                    assert_eq!(back, v);
                }
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
