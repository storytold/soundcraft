//! `soundcraft-cli info FILE` on an audio file, run as the real binary.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

use serde_json::Value;

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&(data.len() as u32).to_le_bytes());
    c.extend_from_slice(data);
    if data.len() % 2 == 1 {
        c.push(0);
    }
    c
}

/// A 16-bit mono WAV of 2 frames carrying `ixml`.
fn wav_with_ixml(ixml: &str) -> Vec<u8> {
    let mut fmt = Vec::new();
    for v in [1u16, 1] {
        fmt.extend_from_slice(&v.to_le_bytes());
    }
    fmt.extend_from_slice(&48_000u32.to_le_bytes());
    fmt.extend_from_slice(&96_000u32.to_le_bytes());
    for v in [2u16, 16] {
        fmt.extend_from_slice(&v.to_le_bytes());
    }
    let mut body = b"WAVE".to_vec();
    body.extend(chunk(b"fmt ", &fmt));
    body.extend(chunk(b"data", &[0; 4]));
    body.extend(chunk(b"iXML", ixml.as_bytes()));
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

fn info(name: &str, bytes: &[u8]) -> Value {
    let dir = std::env::temp_dir().join(format!("sc-cli-info-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("take.wav");
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_soundcraft-cli")).env("SOUNDCRAFT_NO_PREFS", "1").arg("info").arg(&path).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn info_shows_the_ixml_of_a_field_recording() {
    let xml = "<BWFXML><PROJECT>Doc</PROJECT><SCENE>7</SCENE><TAKE>2</TAKE><TAPE>T1</TAPE><NOTE>wind</NOTE><CIRCLED>TRUE</CIRCLED>\
               <TRACK_LIST><TRACK><CHANNEL_INDEX>1</CHANNEL_INDEX><INTERLEAVE_INDEX>1</INTERLEAVE_INDEX><NAME>Boom</NAME></TRACK></TRACK_LIST></BWFXML>";
    let v = info("ok", &wav_with_ixml(xml));
    let ixml = &v["ixml"];
    assert_eq!(ixml["project"], "Doc");
    assert_eq!(ixml["scene"], "7");
    assert_eq!(ixml["take"], "2");
    assert_eq!(ixml["tape"], "T1");
    assert_eq!(ixml["note"], "wind");
    assert_eq!(ixml["circled"], true);
    assert_eq!(ixml["tracks"][0]["name"], "Boom");
    assert_eq!(ixml["tracks"][0]["channel_index"], 1);
    assert_eq!(v["frames"], 2);
}

#[test]
fn info_still_reads_a_file_with_broken_ixml() {
    let v = info("broken", &wav_with_ixml("<BWFXML><SCENE>7</TAKE>"));
    assert_eq!(v.get("ixml"), Some(&Value::Null));
    assert_eq!(v["frames"], 2);
}
