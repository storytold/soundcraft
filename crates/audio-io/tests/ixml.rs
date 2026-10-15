//! iXML (field-recorder metadata) read from WAV / BWF / RF64 files. A bad iXML chunk is ignored:
//! it never costs the audio.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::{IxmlInfo, IxmlTrack, decode, probe};

const RECORDER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- written by a field recorder -->
<BWFXML>
  <IXML_VERSION>2.10</IXML_VERSION>
  <PROJECT>Night &amp; Day</PROJECT>
  <SCENE>12A</SCENE>
  <TAKE>3</TAKE>
  <TAPE>D001</TAPE>
  <CIRCLED>TRUE</CIRCLED>
  <NOTE><![CDATA[boom <off> mic]]> &#233;t&#xE9;</NOTE>
  <SPEED><MASTER_SPEED>48000/1001</MASTER_SPEED></SPEED>
  <TRACK_LIST>
    <TRACK_COUNT>2</TRACK_COUNT>
    <TRACK>
      <CHANNEL_INDEX>1</CHANNEL_INDEX>
      <INTERLEAVE_INDEX>1</INTERLEAVE_INDEX>
      <NAME>Boom</NAME>
      <FUNCTION>LEFT</FUNCTION>
    </TRACK>
    <TRACK><CHANNEL_INDEX>2</CHANNEL_INDEX><INTERLEAVE_INDEX>2</INTERLEAVE_INDEX><NAME>Lav Anna</NAME></TRACK>
  </TRACK_LIST>
</BWFXML>
"#;

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&(data.len() as u32).to_le_bytes());
    c.extend_from_slice(data);
    if data.len() % 2 == 1 {
        c.push(0);
    }
    c
}

/// A 16-bit stereo WAV of 4 frames with `extra` chunks after the audio (where recorders put iXML).
fn wav(extra: &[u8]) -> Vec<u8> {
    let mut fmt = Vec::new();
    for v in [1u16, 2] {
        fmt.extend_from_slice(&v.to_le_bytes());
    }
    fmt.extend_from_slice(&48_000u32.to_le_bytes());
    fmt.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    for v in [4u16, 16] {
        fmt.extend_from_slice(&v.to_le_bytes());
    }
    let mut body = b"WAVE".to_vec();
    body.extend(chunk(b"fmt ", &fmt));
    body.extend(chunk(b"data", &[0x10; 16]));
    body.extend_from_slice(extra);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

fn with_ixml(xml: &[u8]) -> Vec<u8> {
    wav(&chunk(b"iXML", xml))
}

/// The audio reads as it would without the iXML chunk, and the iXML is `None`.
fn ignored(bytes: &[u8]) {
    let info = probe(bytes, None).unwrap();
    assert_eq!(info.ixml, None);
    assert_eq!((info.channels, info.frames), (2, 4));
    let (dinfo, buf) = decode(bytes, None).unwrap();
    assert_eq!(dinfo.ixml, None);
    assert_eq!(buf, decode(&wav(&[]), None).unwrap().1);
}

#[test]
fn reads_the_recorder_fields_and_track_names() {
    let bytes = with_ixml(RECORDER.as_bytes());
    let want = IxmlInfo {
        project: "Night & Day".into(),
        scene: "12A".into(),
        take: "3".into(),
        tape: "D001".into(),
        note: "boom <off> mic été".into(),
        circled: true,
        tracks: vec![
            IxmlTrack { channel_index: Some(1), interleave_index: Some(1), name: "Boom".into() },
            IxmlTrack { channel_index: Some(2), interleave_index: Some(2), name: "Lav Anna".into() },
        ],
    };
    assert_eq!(probe(&bytes, None).unwrap().ixml.as_deref(), Some(&want));
    assert_eq!(decode(&bytes, None).unwrap().0.ixml.as_deref(), Some(&want));
}

#[test]
fn reads_ixml_before_the_audio_and_in_rf64() {
    let xml = "<BWFXML><SCENE>4</SCENE><TAKE>1</TAKE></BWFXML>";
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
    body.extend(chunk(b"ds64", &[0; 28]));
    body.extend(chunk(b"fmt ", &fmt));
    body.extend(chunk(b"iXML", xml.as_bytes()));
    body.extend(chunk(b"data", &[0; 8]));
    let mut rf64 = b"RF64".to_vec();
    rf64.extend_from_slice(&u32::MAX.to_le_bytes());
    rf64.extend(body);
    let info = probe(&rf64, None).unwrap();
    let ixml = info.ixml.unwrap();
    assert_eq!((ixml.scene.as_str(), ixml.take.as_str(), ixml.circled), ("4", "1", false));
    assert!(ixml.tracks.is_empty());
}

#[test]
fn trailing_padding_after_the_document_is_fine() {
    let mut xml = RECORDER.as_bytes().to_vec();
    xml.extend_from_slice(&[0; 9]);
    assert_eq!(probe(&with_ixml(&xml), None).unwrap().ixml.unwrap().scene, "12A");
}

#[test]
fn no_ixml_chunk_means_none() {
    assert_eq!(probe(&wav(&[]), None).unwrap().ixml, None);
}

#[test]
fn malformed_ixml_is_ignored_not_fatal() {
    for xml in [
        "<BWFXML><SCENE>1</TAKE></BWFXML>",
        "<BWFXML><SCENE>1</SCENE>",
        "<BWFXML><NOTE>a &bogus; b</NOTE></BWFXML>",
        "<BWFXML><NOTE>a & b</NOTE></BWFXML>",
        "<BWFXML><SCENE a=\"1>2</SCENE></BWFXML>",
        "<BWFXML></BWFXML><BWFXML></BWFXML>",
        "<NOT_IXML><SCENE>1</SCENE></NOT_IXML>",
        "<!DOCTYPE BWFXML [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><BWFXML><NOTE>&x;</NOTE></BWFXML>",
        "<BWFXML><NOTE>&#0;</NOTE></BWFXML>",
        "<BWFXML><NOTE>&#x110000;</NOTE></BWFXML>",
        "",
        "not xml at all",
    ] {
        ignored(&with_ixml(xml.as_bytes()));
    }
    let deep = format!("<BWFXML>{}{}</BWFXML>", "<A>".repeat(10_000), "</A>".repeat(10_000));
    ignored(&with_ixml(deep.as_bytes()));
}

#[test]
fn xml_cut_short_is_ignored() {
    let cut = &RECORDER.as_bytes()[..RECORDER.len() / 2];
    ignored(&with_ixml(cut));
}

#[test]
fn a_truncated_ixml_chunk_is_ignored() {
    // The chunk claims more bytes than the file holds.
    let mut c = chunk(b"iXML", RECORDER.as_bytes());
    c.truncate(c.len() - 40);
    ignored(&wav(&c));
}

#[test]
fn an_oversized_ixml_chunk_is_ignored() {
    let mut xml = "<BWFXML><SCENE>1</SCENE><NOTE>".to_string();
    xml.push_str(&"x".repeat(1 << 20));
    xml.push_str("</NOTE></BWFXML>");
    ignored(&with_ixml(xml.as_bytes()));
}
