# OmniuSong project archive

The `.omniusong` file is a complete backup of one accepted project and its referenced audio. Restoring creates a new project, preserving the source session. It includes original recordings and intermediate audio referenced by transcription provenance, plus notes, expression estimates, locks, lyrics, patches, and project metadata.

Model weights, credentials, provider accounts, pending generation jobs, the global model selection list, and application settings are outside this format. Accepted-candidate provenance embedded in the project is preserved. A restored project can reference its original job IDs as historical provenance without recreating or rerunning those jobs.

## Version 1 format

The file is gzip-compressed UTF-8 JSON. Its MIME type is `application/vnd.omniusong.project+gzip`. It has no extracted paths, scripts, or executable entries.

```json
{
  "format": "omniusong-project-archive",
  "version": 1,
  "exportedAt": "2026-10-15T07:51:46.222Z",
  "project": {},
  "assets": [
    {
      "id": "<sha256>.wav",
      "byteLength": 1234,
      "encoding": "base64",
      "data": "<canonical base64>"
    }
  ]
}
```

`project` is the complete versioned OmniuSong project, including unknown metadata. Each audio ID uses the SHA256 of its bytes followed by `wav`, `webm`, `mp4`, or `ogg`. The archive scanner collects content IDs throughout the project, including accepted provenance and source references, and writes each unique asset once.

## Validation and limits

| Limit | Version 1 preview |
| --- | --- |
| Compressed upload or download | 48 MiB |
| Expanded JSON | 72 MiB |
| Total decoded audio | 48 MiB |
| Individual audio file | 30 MiB |
| Unique audio files | 128 |
| Project JSON | 1,900,000 bytes, leaving room for restore metadata and the control-service request wrapper |
| Metadata traversal | At most 64 nested levels and 250,000 visited values |

Export fails if referenced audio is absent or damaged. Import rejects unknown archive versions, invalid project data, unsafe asset IDs, duplicates, unreferenced audio entries, missing referenced audio, inconsistent lengths, noncanonical base64, failed checksums, and excessive expansion or size.

The importer validates the entire bundle before writing assets. It commits complete audio bytes atomically and publishes the new project last. An interrupted import may leave reusable audio in the content store, but does not publish a partially restored session. Importing validated bytes can repair a damaged local file with the same content address.

The restored project receives a fresh ID, revision zero, a “restored” suffix, and an `importedFrom` record containing source identity and revision. Track IDs, note IDs, musical data, unknown metadata, and the accepted history remain intact within the new project.

## Service routes

- `GET /api/projects/:id/archive` exports a complete gzip bundle.
- `POST /api/archives` accepts the archive MIME type and returns the new project, asset count, and archive version.

The service remains bound to loopback with its existing origin checks. Restoring a backup has no model execution or external network effect.

## Verified workflow

The browser exported an actual 446,150-byte project with eight tracks, 95 notes, and two audio assets. The downloaded file was restored through the running local service, and musical data, accepted history, and audio bytes matched. The source session stayed unchanged. A note added to the restored copy survived switching sessions and reloading the studio.

Automated file selection in Chrome is gated by the ChatGPT extension's file URL permission. The file chooser and download were exercised; selection of a restore file through that automation interface remains pending. Native Mac file dialogs and download handling are also pending qualification. The HTTP restore path is covered by automated checks and the live round-trip receipt.

Optional note input metadata and `inputCapture` are included exactly with the project. Native host timestamps remain strings, and raw packet words stay unsigned integers. Archive tests include source/channel/group, physical key duration, release velocity, and raw UMP with a 64-bit host timestamp.

Imported originals can use WAV, WebM, MP4/M4A, Ogg, MP3, or FLAC content addresses. The archive includes import origin, original filename, duration, MIME type, and byte length. Browser decoding support is separate from byte preservation and backup integrity. The complete browser WAV import/transcription archive was restored with both original and derived analysis audio intact; see `verification/audio-import.json`.

Generated audio tracks can declare `audio.origin: "generated"`. Their exact model WAV bytes, source note guide, and model/runtime evidence are retained independently of microphone and imported-human audio. This origin survives validated complete backups.
