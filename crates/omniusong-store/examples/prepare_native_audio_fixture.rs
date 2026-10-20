//! Create explicitly synthetic music for an actual native output walkthrough.
use omniusong_core::ProjectDocument;
use omniusong_store::NativeStore;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Usage: prepare_native_audio_fixture EXISTING_STORE NEW_RECEIPT".into());
    }
    let receipt_path = PathBuf::from(&args[1]);
    if receipt_path.exists() {
        return Err("Receipt already exists".into());
    }
    let mut store = NativeStore::open_existing(&args[0])?;
    let mut value = ProjectDocument::new("Native audio · synthetic playback check")?
        .value()
        .clone();
    value["tempo"] = json!(120);
    value["tracks"][0]["name"] = json!("Procedural test phrase · not a human performance");
    value["tracks"][0]["gain"] = json!(0.2);
    value["tracks"][0]["notes"] = json!([
        {"id":"fixture-c4","pitch":60,"startTick":0,"durationTicks":720,"velocity16":40000,"expression":[]},
        {"id":"fixture-e4","pitch":64,"startTick":960,"durationTicks":720,"velocity16":40000,"expression":[]},
        {"id":"fixture-g4","pitch":67,"startTick":1920,"durationTicks":720,"velocity16":40000,"expression":[]},
        {"id":"fixture-c5","pitch":72,"startTick":2880,"durationTicks":1440,"velocity16":40000,"expression":[]}
    ]);
    value["omniusongAudioQualification"] = json!({"version":1,"origin":"procedural_fixture","humanPerformance":false,"purpose":"native device playback and local recording walkthrough"});
    let project = store.create(&ProjectDocument::from_value(value)?)?;
    let bytes = project.to_json()?;
    let receipt = json!({"format":"omniusong.native-audio-ui-fixture","version":1,"projectId":project.id(),"revision":project.revision(),"documentSha256":format!("{:x}",Sha256::digest(&bytes)),"origin":"procedural test notes","originalSessionsReplaced":false});
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(receipt_path)?;
    use std::io::Write;
    output.write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?;
    output.sync_all()?;
    println!("{}", project.id());
    Ok(())
}
