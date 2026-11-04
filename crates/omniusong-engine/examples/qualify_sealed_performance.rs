//! Read-only qualification of a previously sealed software-output original.
//! This opens no audio device and never seals, recovers, resumes or attaches it.
use omniusong_engine::performance_writer::inspect_sealed_performance;
use omniusong_recording::{CaptureReader, SealedJournal};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn digest(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let len = file.read(&mut buffer)?;
        if len == 0 {
            break;
        }
        hash.update(&buffer[..len]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: qualify_sealed_performance SOURCE RECEIPT.json".into());
    }
    let path = fs::canonicalize(PathBuf::from(&args[0]))?;
    let receipt_path = PathBuf::from(&args[1]);
    let before = digest(&path)?;
    let names_before: Vec<_> = fs::read_dir(path.parent().ok_or("source has no directory")?)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()?;
    let source = SealedJournal::open(&path)?;
    let snapshot = source.inspect_snapshot()?;
    let semantic = inspect_sealed_performance(&source)?;
    let manifest = source.manifest();
    assert_eq!(snapshot.manifest.as_ref(), Some(manifest));
    assert_eq!(snapshot.descriptor_sha256, semantic.descriptor_sha256);
    assert_eq!(snapshot.original_rows_sha256, semantic.original_rows_sha256);
    assert_eq!(snapshot.journal_sha256, before);
    let selected = semantic
        .selection
        .as_ref()
        .ok_or("no complete checkpoint")?;
    assert_eq!(selected.chunk_count, manifest.body.chunk_count);
    assert_eq!(
        selected.chunk_index_sha256,
        manifest.body.chunk_index_sha256
    );
    assert!(semantic.problem.is_none());
    assert!(semantic.capture_fault.is_none() && !semantic.capture_fault_pending);
    assert!(
        manifest.body.interrupted.is_none(),
        "this qualifier expects a normal original"
    );
    assert!(semantic.finish_cut.is_some());
    let mut owner_name = path.as_os_str().to_owned();
    owner_name.push(".owner.lock");
    let owner_exists = PathBuf::from(owner_name).exists();
    if !owner_exists {
        assert!(
            CaptureReader::open(&path).is_err(),
            "ordinary recovery reader requires its existing owner"
        );
    }
    let receipt = json!({
        "format":"omniusong.sealed-performance-inspection-qualification", "version":1,
        "source":path, "sourceSha256":before, "sourceBytes":fs::metadata(&path)?.len(),
        "manifestSha256":manifest.sha256, "descriptorSha256":snapshot.descriptor_sha256,
        "originalRowsSha256":snapshot.original_rows_sha256, "selected":selected,
        "semantic":semantic, "ownerSidecarExisted":owner_exists,
        "fullRetainedOriginalAuthenticated":true, "sourceUnchanged":true,
        "createdOwnerOrSidecar":false, "deviceOpened":false, "nativeInputOpened":false,
        "normalSealAuthorityObtained":false, "attached":false,
        "qualifierSourceSha256":format!("{:x}",Sha256::digest(include_bytes!("qualify_sealed_performance.rs")))
    });
    drop(source);
    assert_eq!(digest(&path)?, before);
    let mut names_after: Vec<_> = fs::read_dir(path.parent().ok_or("source has no directory")?)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()?;
    let mut names_before = names_before;
    names_before.sort();
    names_after.sort();
    assert_eq!(names_before, names_after);
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(receipt_path)?;
    output.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    output.write_all(b"\n")?;
    println!(
        "Verified sealed performance without devices, writes to originals, or owner sidecars."
    );
    Ok(())
}
