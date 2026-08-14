use nodegraph_hotreload::{load_species, SpeciesFile, SPECIES_VERSION};
use nodegraph_ir::TreeSpecies;

#[test]
fn a_sparse_species_file_loads() {
    // The authoring property: a species declares what differs, and a field
    // added later cannot break a file already on disk.
    let dir = std::env::temp_dir().join("vx_species_sparse");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("oak.species.json"),
        r#"{"version":1,"name":"oak","species":{"trunk_radius":2.0,"max_lobes":7}}"#,
    )
        .unwrap();
    let s = load_species(&dir, "oak").unwrap();
    assert_eq!(s.trunk_radius, 2.0);
    assert_eq!(s.max_lobes, 7);
    assert_eq!(s.internode, TreeSpecies::default().internode, "unstated fields default");
}

#[test]
fn a_species_file_naming_something_else_is_rejected() {
    // A name that disagrees with its filename is how two species quietly
    // become one.
    let dir = std::env::temp_dir().join("vx_species_misnamed");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("oak.species.json"),
        r#"{"version":1,"name":"birch","species":{}}"#,
    )
        .unwrap();
    assert!(load_species(&dir, "oak").is_err());
}

#[test]
fn a_future_version_is_refused_rather_than_guessed_at() {
    let dir = std::env::temp_dir().join("vx_species_future");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("oak.species.json"),
        format!(r#"{{"version":{},"name":"oak","species":{{}}}}"#, SPECIES_VERSION + 1),
    )
        .unwrap();
    assert!(load_species(&dir, "oak").is_err());
}

#[test]
fn a_saved_species_loads_back_identically() {
    let dir = std::env::temp_dir().join("vx_species_roundtrip");
    let _ = std::fs::remove_dir_all(&dir);
    let species = TreeSpecies { trunk_radius: 2.4, max_lobes: 5, wobble: 0.32,
                                ..TreeSpecies::default() };
    SpeciesFile { version: SPECIES_VERSION, name: "birch".into(), species }
        .save(&dir)
        .unwrap();
    assert_eq!(load_species(&dir, "birch").unwrap(), species);
}

#[test]
fn a_species_name_that_is_a_path_is_refused() {
    let dir = std::env::temp_dir().join("vx_species_evil");
    let f = SpeciesFile { version: SPECIES_VERSION, name: "../../evil".into(),
                          species: TreeSpecies::default() };
    assert!(f.save(&dir).is_err());
}
