//! In-memory sealed-recording retry identity; no journal or engine access.

use super::*;

fn fixture_root() -> PathBuf {
    PathBuf::from(if cfg!(windows) {
        r"C:\synthetic-capture-request"
    } else {
        "/synthetic-capture-request"
    })
}

fn retained_audio() -> (AudioUi, PathBuf, TakePublication) {
    let mut audio = AudioUi::new();
    let mut take = test_support::take();
    take.journal_path = fixture_root().join("retained.sqlite3");
    audio.target = Some(CaptureTarget {
        project_id: "synthetic-project".into(),
        revision: 7,
    });
    audio.original = Some(CaptureIdentity {
        journal_path: take.journal_path.clone(),
        take_id: take.take_id.clone(),
    });
    audio.captured(take);
    let sealed = audio.sealed.as_ref().expect("valid synthetic sealed take");
    assert!(sealed.frozen_request.is_none());
    let journal = sealed.take.journal_path.clone();
    let publication = sealed.publication.clone();
    (audio, journal, publication)
}

#[test]
fn retry_after_current_studio_changes_keeps_the_entire_original_request() {
    let (mut audio, journal, publication) = retained_audio();
    let origin = fixture_root().join("first-studio");
    let first = audio
        .request_for_publication(&origin, &journal, &publication)
        .unwrap();
    let first_bytes = first.canonical_bytes().unwrap();
    let first_sha = first.fingerprint().unwrap();
    assert_eq!(first.version, 1);
    assert_eq!(first.origin_root.as_os_str(), origin.as_os_str());
    assert_eq!(first.journal_path.as_os_str(), journal.as_os_str());
    assert_eq!(first.publication, publication);

    audio.publication_started();
    audio.publication_failed();
    let current_origin = fixture_root().join("different-current-studio");
    let (retry_journal, retry_publication) = audio.retry_publication().unwrap();
    let retry = audio
        .request_for_publication(&current_origin, &retry_journal, &retry_publication)
        .unwrap();
    assert_eq!(retry.canonical_bytes().unwrap(), first_bytes);
    assert_eq!(retry.fingerprint().unwrap(), first_sha);
    assert_eq!(retry.origin_root.as_os_str(), origin.as_os_str());
    assert_ne!(retry.origin_root.as_os_str(), current_origin.as_os_str());
    assert_eq!(
        audio.sealed.as_ref().unwrap().frozen_request.as_ref(),
        Some(&first)
    );
    assert_eq!(
        audio.original.as_ref().unwrap().journal_path.as_os_str(),
        journal.as_os_str()
    );
}

#[test]
fn altered_locator_or_publication_cannot_replace_frozen_request_or_retained_original() {
    let (mut audio, journal, publication) = retained_audio();
    let origin = fixture_root().join("first-studio");
    let first = audio
        .request_for_publication(&origin, &journal, &publication)
        .unwrap();
    let first_bytes = first.canonical_bytes().unwrap();
    let original_take_id = audio.original.as_ref().unwrap().take_id.clone();
    let wrong_journal = fixture_root().join("other-recording.sqlite3");
    assert!(
        audio
            .request_for_publication(&origin, &wrong_journal, &publication)
            .is_err()
    );
    let mut changed_operation = publication.clone();
    changed_operation.operation_id = OperationId("replacement-recording-save".into());
    let mut changed_revision = publication.clone();
    changed_revision.expected_revision += 1;
    let mut changed_placement = publication.clone();
    changed_placement.clip.position.frame += 1;
    for altered in [changed_operation, changed_revision, changed_placement] {
        assert!(
            audio
                .request_for_publication(&origin, &journal, &altered)
                .is_err()
        );
    }

    let sealed = audio.sealed.as_ref().unwrap();
    let retained = sealed.frozen_request.as_ref().unwrap();
    assert_eq!(retained.canonical_bytes().unwrap(), first_bytes);
    assert_eq!(sealed.publication, publication);
    assert_eq!(sealed.take.journal_path.as_os_str(), journal.as_os_str());
    assert_eq!(sealed.take.take_id, original_take_id);
    let original = audio.original.as_ref().unwrap();
    assert_eq!(original.journal_path.as_os_str(), journal.as_os_str());
    assert_eq!(original.take_id, original_take_id);
    let exact_retry = audio
        .request_for_publication(&origin, &journal, &publication)
        .unwrap();
    assert_eq!(exact_retry.canonical_bytes().unwrap(), first_bytes);
}
