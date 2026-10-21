//! Read-only bounded inspection of an existing synthetic qualification original.
//! Opens no audio device, catalog, writer, recovery path, or publication action.
use omniusong_engine::performance_writer::{
    inspect_performance_bounded, inspect_sealed_performance_bounded,
};
use omniusong_recording::{CaptureReader, InspectionBudget, InspectionLimits, SealedJournal};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

fn digest(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn names(path: &Path) -> Result<Vec<std::ffi::OsString>, std::io::Error> {
    let mut values = fs::read_dir(path)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    values.sort();
    Ok(values)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: qualify_bounded_performance capture|sealed SOURCE RECEIPT.json".into());
    }
    let path = fs::canonicalize(PathBuf::from(&args[1]))?;
    let parent = path.parent().ok_or("source has no parent")?;
    let before = digest(&path)?;
    let names_before = names(parent)?;
    let limits = InspectionLimits::default();
    let budget = InspectionBudget::new(limits, Arc::new(AtomicBool::new(false)))?;
    let start = Instant::now();
    let (snapshot, performance) = if args[0] == "capture" {
        let reader = CaptureReader::open_bounded(&path, &budget)?;
        (
            reader.inspection().clone(),
            inspect_performance_bounded(&reader)?,
        )
    } else if args[0] == "sealed" {
        let reader = SealedJournal::open_bounded(&path, &budget)?;
        let snapshot = reader.inspect_snapshot()?;
        (snapshot, inspect_sealed_performance_bounded(&reader)?)
    } else {
        return Err("reader must be capture or sealed".into());
    };
    let seconds = start.elapsed().as_secs_f64();
    let after = digest(&path)?;
    assert_eq!(before, after);
    assert_eq!(before, snapshot.journal_sha256);
    assert_eq!(names_before, names(parent)?);
    assert_eq!(snapshot.descriptor_sha256, performance.descriptor_sha256);
    assert_eq!(
        snapshot.original_rows_sha256,
        performance.original_rows_sha256
    );
    let receipt = json!({"reader":args[0].to_string_lossy(),"source":path,"limits":limits,"usage":budget.usage(),"seconds":seconds,"journal":snapshot,"performance":performance,"sourceBytesUnchanged":true,"directoryNamesUnchanged":true,"scope":"Read-only bounded source/semantic inspection. No audio device, input, capture, recovery, publication, inference, listening or native lifetime authority."});
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(PathBuf::from(&args[2]))?;
    output.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    output.write_all(b"\n")?;
    println!(
        "{}",
        json!({"seconds":seconds,"usage":budget.usage(),"unchanged":true})
    );
    Ok(())
}
