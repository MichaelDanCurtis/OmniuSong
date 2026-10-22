//! Read-only reconciliation of an already committed performance attachment.
use crate::{NativeStore, Result, StoreError, TakePublication};
use crate::{operations::replay_operation, takes::publication_request};
use omniusong_core::native::{NATIVE_SCHEMA_VERSION, TakeClip};

impl NativeStore {
    /// Exact historical publication proof, without opening any take file.
    pub fn take_publication_receipt(
        &self,
        intent: &TakePublication,
    ) -> Result<Option<crate::EditReceipt>> {
        let tx = self.connection.unchecked_transaction()?;
        let receipt = checked_publication_receipt(&tx, intent)?;
        tx.rollback()?;
        Ok(receipt)
    }

    /// Historical commitment, independent of current placement/availability.
    pub fn performance_attachment_committed(&self, intent: &TakePublication) -> Result<bool> {
        Ok(self.take_publication_receipt(intent)?.is_some())
    }
}

pub(crate) fn checked_publication_receipt(
    connection: &rusqlite::Connection,
    intent: &TakePublication,
) -> Result<Option<crate::EditReceipt>> {
    let request = publication_request(intent)?;
    let Some(receipt) = replay_operation(
        connection,
        &intent.operation_id.0,
        &intent.project_id.0,
        intent.expected_revision,
        &request,
    )?
    else {
        return Ok(None);
    };
    // A checksum authenticates stored bytes, not their agreement with the
    // requested destination. Keep this check scoped to the historical
    // postimage; later edits must not change the commitment answer.
    let track = receipt
        .document
        .tracks()
        .iter()
        .find(|track| track["id"] == intent.track_id.0);
    let clip = track
        .filter(|track| track["kind"] == "recording" && track["locked"] == false)
        .and_then(|track| track["recording"]["clips"].as_array())
        .and_then(|clips| clips.iter().find(|clip| clip["id"] == intent.clip.id));
    let clip: Option<TakeClip> = clip
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
    if receipt.document.schema_version() != NATIVE_SCHEMA_VERSION
        || clip.as_ref() != Some(&intent.clip)
    {
        return Err(StoreError::Invalid(
            "Stored attachment result differs from its requested track or clip".into(),
        ));
    }
    // An existing recording track retains its prior name; track_name in a
    // publication is a creation default, not an unconditional rename.
    Ok(Some(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum;
    use omniusong_core::{
        ProjectDocument,
        contracts::{OperationId, ProjectId, TrackId},
        native::{TakeRange, TakeReference, TimelinePosition},
    };
    use omniusong_recording::*;
    use rusqlite::params;
    use std::{collections::BTreeMap, fs, path::PathBuf};

    struct Fixture {
        root: PathBuf,
        store: NativeStore,
        source: PathBuf,
        intent: TakePublication,
    }
    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/performance-attachment-check")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&root).unwrap();
            let source = root.join("source.sqlite3");
            let stamp = HostTimestamp {
                domain: ClockDomainId("synthetic-clock".into()),
                epoch: ClockEpochId("epoch".into()),
                ticks: 0,
            };
            let descriptor = TakeDescriptor {
                version: 1,
                take_id: TakeId("original".into()),
                streams: vec![StreamDescriptor {
                    stream_id: "audio".into(),
                    segment_id: "segment".into(),
                    source_id: "synthetic-output".into(),
                    source_name: "Synthetic output".into(),
                    source_connection_generation: 1,
                    host_clock: HostClock {
                        domain: stamp.domain.clone(),
                        epoch: stamp.epoch.clone(),
                        ticks_per_second_numerator: 48_000,
                        ticks_per_second_denominator: 1,
                    },
                    kind: StreamKind::Audio {
                        format: PcmFormat {
                            sample_rate: 48_000,
                            channels: 1,
                            encoding: PcmEncoding::F32Le,
                            layout: PcmLayout::Interleaved,
                        },
                        audio_epoch: stamp.epoch.clone(),
                    },
                    initial_source_position: 0,
                }],
                provenance: BTreeMap::new(),
            };
            let mut journal = Journal::create(&source, &descriptor).unwrap();
            journal
                .append(
                    &ChunkHeader {
                        version: 1,
                        take_id: descriptor.take_id.clone(),
                        stream_id: "audio".into(),
                        segment_id: "segment".into(),
                        sequence: 0,
                        range: SourceRange::AudioFrames { start: 0, count: 2 },
                        interpretation: descriptor.streams[0].kind.clone(),
                        observed_start: stamp.clone(),
                        observed_end: stamp,
                        clock_anchors: vec![],
                        kind: ChunkKind::Content,
                    },
                    &[0; 8],
                )
                .unwrap();
            let manifest = journal.seal("synthetic-seal").unwrap();
            drop(journal);
            let mut store = NativeStore::open(root.join("store")).unwrap();
            let project = store
                .create(&ProjectDocument::new("Target").unwrap())
                .unwrap();
            let intent = TakePublication {
                operation_id: OperationId("attachment-once".into()),
                project_id: ProjectId(project.id().into()),
                expected_revision: 0,
                track_id: TrackId("original-track".into()),
                track_name: "Original recording".into(),
                clip: TakeClip {
                    id: "original-clip".into(),
                    source: TakeReference {
                        take_id: descriptor.take_id,
                        manifest_sha256: manifest.sha256,
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames { start: 0, count: 2 },
                    position: TimelinePosition {
                        frame: 0,
                        sample_rate: 48_000,
                    },
                    fades: None,
                    edit_source: None,
                },
            };
            Self {
                root,
                store,
                source,
                intent,
            }
        }
        fn commit(&mut self) -> ProjectDocument {
            self.store
                .attach_capture(&self.source, &self.intent)
                .unwrap()
                .document
        }
        fn replace_result(&self, mutate: impl FnOnce(&mut serde_json::Value)) {
            let result: String = self
                .store
                .connection
                .query_row(
                    "SELECT result_json FROM operations WHERE operation_id=?1",
                    [&self.intent.operation_id.0],
                    |row| row.get(0),
                )
                .unwrap();
            let mut value: serde_json::Value = serde_json::from_str(&result).unwrap();
            mutate(&mut value);
            let bytes = serde_json::to_vec(&value).unwrap();
            // Deliberately preserve a valid document and recompute its checksum:
            // rejection must come from semantic receipt binding, not bad JSON.
            ProjectDocument::from_json(&bytes).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            self.store
                .connection
                .execute(
                    "UPDATE operations SET result_json=?1,result_sha256=?2 WHERE operation_id=?3",
                    params![text, checksum(text.as_bytes()), self.intent.operation_id.0],
                )
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    #[test]
    fn absent_receipt_does_not_attach_or_change_catalog_bytes() {
        let f = Fixture::new();
        let catalog = f.root.join("store/studio.sqlite3");
        let before = fs::read(&catalog).unwrap();
        assert!(!f.store.performance_attachment_committed(&f.intent).unwrap());
        assert_eq!(fs::read(&catalog).unwrap(), before);
        assert!(
            f.store
                .registered_take(&f.intent.clip.source.take_id)
                .is_err()
        );
        assert_eq!(f.store.load(&f.intent.project_id.0).unwrap().revision(), 0);
        assert!(!catalog.with_extension("sqlite3-journal").exists());
    }

    #[test]
    fn committed_receipt_survives_later_edits_and_unavailable_physical_sources() {
        let mut f = Fixture::new();
        let mut later = f.commit();
        later.rename("Later project revision").unwrap();
        let latest = f.store.save(&later, 1).unwrap();
        let object = f
            .store
            .registered_take(&f.intent.clip.source.take_id)
            .unwrap();
        fs::remove_file(&f.source).unwrap();
        fs::remove_file(
            f.root
                .join("store/takes")
                .join(format!("{}.sqlite3", object.object_sha256)),
        )
        .unwrap();
        let catalog = f.root.join("store/studio.sqlite3");
        let before = fs::read(&catalog).unwrap();
        assert!(f.store.performance_attachment_committed(&f.intent).unwrap());
        assert_eq!(fs::read(&catalog).unwrap(), before);
        // Metadata-only receipt verification must not install its old snapshot.
        let revision: i64 = f
            .store
            .connection
            .query_row(
                "SELECT revision FROM projects WHERE id=?1",
                [latest.id()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision, 2);
    }

    #[test]
    fn existing_track_name_is_not_mistaken_for_a_rename_postcondition() {
        let mut f = Fixture::new();
        let first = f.commit();
        f.intent.operation_id.0 = "second-attachment".into();
        f.intent.expected_revision = first.revision();
        f.intent.track_name = "Unused creation default".into();
        f.intent.clip.id = "second-clip".into();
        let second = f.commit();
        assert_eq!(
            second.tracks().last().unwrap()["name"],
            "Original recording"
        );
        assert!(f.store.performance_attachment_committed(&f.intent).unwrap());
    }

    #[test]
    fn same_operation_different_intent_is_an_error_and_leaves_source_unchanged() {
        let mut f = Fixture::new();
        f.commit();
        let original = fs::read(&f.source).unwrap();
        for change in 0..3 {
            let mut other = f.intent.clone();
            match change {
                0 => other.expected_revision += 1,
                1 => other.track_name = "Different request".into(),
                _ => other.clip.position.frame = 1,
            }
            assert!(f.store.performance_attachment_committed(&other).is_err());
        }
        assert_eq!(fs::read(&f.source).unwrap(), original);
        assert!(f.store.performance_attachment_committed(&f.intent).unwrap());
    }

    #[test]
    fn rehashed_valid_wrong_track_clip_or_revision_cannot_prove_attachment() {
        for change in 0..4 {
            let mut f = Fixture::new();
            f.commit();
            f.replace_result(|value| {
                let track = value["tracks"].as_array_mut().unwrap().last_mut().unwrap();
                match change {
                    0 => track["id"] = "other-track".into(),
                    1 => track["recording"]["clips"][0]["position"]["frame"] = 1.into(),
                    2 => track["locked"] = true.into(),
                    _ => value["revision"] = 2.into(),
                }
            });
            assert!(f.store.performance_attachment_committed(&f.intent).is_err());
            assert!(f.store.connection.is_autocommit());
        }
    }

    #[test]
    fn damaged_receipt_is_not_absence_and_snapshot_releases_after_error() {
        let mut f = Fixture::new();
        f.commit();
        f.store
            .connection
            .execute(
                "UPDATE operations SET request_sha256=?1 WHERE operation_id=?2",
                params!["0".repeat(64), f.intent.operation_id.0],
            )
            .unwrap();
        assert!(f.store.performance_attachment_committed(&f.intent).is_err());
        assert!(f.store.connection.is_autocommit());
        let mut absent = f.intent.clone();
        absent.operation_id.0 = "never-submitted".into();
        assert!(!f.store.performance_attachment_committed(&absent).unwrap());
    }
}
