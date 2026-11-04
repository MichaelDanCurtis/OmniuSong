//! Read-only source migration qualification. Writes only a new native store and receipt.
use omniusong_core::ProjectDocument;
use omniusong_store::NativeStore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, error::Error, fs, path::PathBuf};

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("Usage: qualify_legacy LEGACY_DATA_ROOT NEW_NATIVE_ROOT RECEIPT_JSON".into());
    }
    let source = PathBuf::from(&args[0]);
    let destination = PathBuf::from(&args[1]);
    let receipt_path = PathBuf::from(&args[2]);
    if destination.exists() {
        return Err(
            "Qualification requires a new native directory; existing sessions were not changed"
                .into(),
        );
    }
    let mut paths: Vec<_> = fs::read_dir(source.join("projects"))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    paths.retain(|p| p.extension().is_some_and(|e| e == "json"));
    paths.sort();
    let mut original_hashes = BTreeMap::new();
    for directory in ["projects", "assets"] {
        for entry in fs::read_dir(source.join(directory))? {
            let path = entry?.path();
            if path.is_file() {
                original_hashes.insert(path.clone(), hash(&fs::read(&path)?));
            }
        }
    }
    let mut store = NativeStore::open(&destination)?;
    let mut projects = vec![];
    for path in paths {
        let raw = fs::read(&path)?;
        let original = ProjectDocument::from_json(&raw)?;
        let imported = store.import_legacy(&path, &source.join("assets"))?;
        let reopened = store.load(imported.id())?;
        assert_eq!(reopened, imported);
        assert_eq!(store.imported_original(imported.id())?, Some(raw.clone()));
        assert_ne!(reopened.id(), original.id());
        for (key, value) in original.value().as_object().unwrap() {
            match key.as_str() {
                "id" | "revision" => {}
                "nativeImport" => {
                    assert_eq!(&reopened.value()["nativeImport"]["previousImport"], value)
                }
                _ => assert_eq!(
                    &reopened.value()[key],
                    value,
                    "Migration changed field {key}"
                ),
            }
        }
        for id in original.referenced_assets()? {
            assert_eq!(
                store.asset(&id)?,
                fs::read(source.join("assets").join(&id))?
            );
        }
        projects.push(json!({"sourceFile":path.file_name().unwrap().to_string_lossy(),"sourceSha256":hash(&raw),"sourceId":original.id(),"nativeId":imported.id(),"tracks":original.tracks().len(),"notes":original.tracks().iter().map(|t|t["notes"].as_array().map_or(0,Vec::len)).sum::<usize>(),"assets":original.referenced_assets()?.len(),"exactOriginalJsonRetained":true,"musicalAndUnknownFieldsEqual":true}));
    }
    let count = projects.len();
    drop(store);
    let reopened_store = NativeStore::open(&destination)?;
    assert_eq!(reopened_store.list()?.len(), count);
    for project in &projects {
        reopened_store.load(project["nativeId"].as_str().unwrap())?;
    }
    for (path, expected) in &original_hashes {
        assert_eq!(
            &hash(&fs::read(path)?),
            expected,
            "Original changed: {}",
            path.display()
        );
    }
    let receipt: Value = json!({"format":"omniusong-native-migration-qualification","version":1,"evidenceDate":"2026-10-20","sourceRoot":source,"nativeRoot":destination,"nativeStoreReopened":true,"sourceFilesVerifiedUnchanged":original_hashes.len(),"projectCount":count,"projects":projects,"limits":"Schema-1 project copy and native persistence only. No native audio, controller, plugin, inference, archive, installer or Windows acceptance is claimed."});
    fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt)?)?;
    println!(
        "Qualified {count} independent native imports; {} original project/audio files unchanged; receipt {}",
        original_hashes.len(),
        receipt_path.display()
    );
    Ok(())
}
