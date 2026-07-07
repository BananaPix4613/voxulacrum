# Changelog

## [Unreleased] — Phase 0

### Changed
- Unified the voxel model on `voxel_core::Voxel` (shape `{Empty, Cube,
  SlabBottom, SlabTop}`, no rotation; 27-bit packed `u32`). Slopes removed.
- Engine chunk storage now holds packed `Voxel` (`u32` palette) instead of bare
  `u16` material ids. Save blob format `BLOB_VERSION` 3 → 4; world disk-cache
  format 4 → 5.

### Migration
- **One-shot save wipe (intentional, pre-release).** On first launch after this
  change, saved chunks under `saves/<world>/` and the mesh cache under
  `cache/meshes/` are cleared when the stored `voxel_format_version` is older
  than the current. Terrain regenerates from seed; no forward migration.
