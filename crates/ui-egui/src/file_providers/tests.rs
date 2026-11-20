use super::*;

fn connected() -> (Host, String) {
    let host = Host::default();
    host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
    let connection = host.connect("test".into(), "Test files".into(), "account-one".into()).unwrap();
    (host, connection)
}

fn request(host: &Host, doc: u64) -> String {
    host.choose(doc, 1, "Example.docx".into(), false).unwrap();
    host.take_events().remove(0).metadata["requestId"].as_str().unwrap().into()
}

fn capture(doc: u64, revision: u64) -> Capture {
    Capture { doc, revision, name: "Example.docx".into(), save_as: false }
}

#[test]
fn disconnect_forgets_document_metadata_before_another_account_registers() {
    let host = Host::default();
    host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
    host.connect("test".into(), "Provider".into(), "account-a".into()).unwrap();
    let documents = json!([{"id":"1","name":"Account A document","dirty":true}]);
    host.publish_documents(documents.clone(), &[1]);
    assert_eq!(host.snapshot()["documents"], documents);
    host.disconnect().unwrap();
    assert!(host.snapshot()["documents"].is_null());
    host.connect("test".into(), "Provider".into(), "account-b".into()).unwrap();
    let snapshot = host.snapshot();
    assert_eq!(snapshot["namespace"], "account-b");
    assert!(snapshot["documents"].is_null(), "the new account must not see metadata cached before its first UI tick");
}

#[test]
fn disabled_until_explicit_registration() {
    let host = Host::default();
    assert!(!host.enabled());
    assert!(!host.connected());
    assert!(host.choose(1, 1, "Example.docx".into(), false).is_err());
    assert!(host.request_open("one").is_err());
    assert!(host.take_events().is_empty());
}

#[test]
fn save_choice_does_not_capture_or_acknowledge() {
    let (host, _) = connected();
    let id = request(&host, 1);
    assert!(host.pending_for(1));
    assert!(host.result(id.clone(), "saved".into(), "one".into()).is_err());
    host.resolve_dialog(&id, "cancel", "").unwrap();
    assert!(!host.busy());
    assert!(!host.bound(1));
}

#[test]
fn repeated_save_has_one_destination_choice() {
    let (host, _) = connected();
    host.choose(1, 1, "Example.docx".into(), false).unwrap();
    host.choose(1, 2, "Example.docx".into(), false).unwrap();
    assert_eq!(host.take_events().len(), 1);
}

#[test]
fn exact_capture_ack_is_consumed_once_and_never_mutates_document() {
    let (host, _) = connected();
    let id = request(&host, 1);
    host.resolve_dialog(&id, "provider", "").unwrap();
    assert!(matches!(host.take_commands().remove(0), Command::Save { doc: 1, .. }));
    host.capture(&id, capture(1, 23), vec![1, 2, 3]).unwrap();
    let event = host.take_events().remove(0);
    assert_eq!(event.bytes, Some(vec![1, 2, 3]));
    assert_eq!(event.metadata["revision"], 23);
    host.result(id.clone(), "saved".into(), "one".into()).unwrap();
    assert!(host.result(id, "saved".into(), "one".into()).is_err());
    assert!(matches!(host.take_commands().remove(0), Command::Result { capture: Capture { doc: 1, revision: 23, .. }, .. }));
    assert!(!host.bound(1), "only the application admits a durable result");
}

#[test]
fn failure_cancel_and_account_changes_do_not_rebind() {
    let (host, _) = connected();
    let id = request(&host, 1);
    assert!(host.disconnect().is_err());
    assert!(host.connect("test".into(), "Test files".into(), "other-account".into()).is_err());
    host.resolve_dialog(&id, "provider", "").unwrap();
    host.take_commands();
    host.capture(&id, capture(1, 1), vec![]).unwrap();
    host.result(id, "failed".into(), String::new()).unwrap();
    assert!(host.disconnect().is_err(), "queued results still belong to the old identity");
    host.take_commands();
    host.disconnect().unwrap();
    assert!(!host.bound(1));
}

#[test]
fn new_connection_rejects_old_list_and_read_result() {
    let (host, connection) = connected();
    host.set_resources(&connection, vec![Resource { id: "one".into(), name: "Example.docx".into(), updated_at: String::new() }]).unwrap();
    let read = host.request_open("one").unwrap();
    host.cancel_open(read.clone(), String::new()).unwrap();
    host.take_events();
    host.disconnect().unwrap();
    let next = host.connect("test".into(), "Test files".into(), "account-one".into()).unwrap();
    assert_ne!(connection, next);
    assert!(host.set_resources(&connection, vec![]).is_err());
    assert!(host.open(read, "Example.docx".into(), vec![]).is_err());
}

#[test]
fn same_resource_alias_is_invalidated_but_homonymous_files_are_not() {
    let (host, _) = connected();
    host.bind(&capture(1, 1), "one".into());
    host.bind(&capture(2, 1), "two".into());
    assert_eq!(host.bind(&capture(3, 1), "one".into()), vec![1]);
    assert!(!host.bound(1));
    assert!(host.bound(2));
    assert!(host.bound(3));
}

#[test]
fn save_as_never_overwrites_binding_by_filename() {
    let (host, _) = connected();
    host.bind(&capture(1, 1), "one".into());
    let id = request(&host, 1);
    host.resolve_dialog(&id, "provider", "").unwrap();
    host.take_commands();
    host.capture(&id, Capture { save_as: true, ..capture(1, 2) }, vec![]).unwrap();
    assert!(host.take_events().remove(0).metadata["resourceId"].is_null());
}

#[test]
fn provider_open_is_correlated_and_committed_after_decode() {
    let (host, connection) = connected();
    host.set_resources(&connection, vec![Resource { id: "one".into(), name: "Example.docx".into(), updated_at: String::new() }]).unwrap();
    let read = host.request_open("one").unwrap();
    assert!(host.open("unknown".into(), "Example.docx".into(), vec![]).is_err());
    host.open(read.clone(), "Example.docx".into(), vec![7]).unwrap();
    assert!(!host.bound(1));
    assert!(host.disconnect().is_err());
    let commands = host.take_commands();
    assert!(matches!(&commands[0], Command::Open { request, resource, bytes, .. } if request == &read && resource == "one" && bytes == &[7]));
    host.opened(&read, Some(capture(1, 1)), "");
    assert!(host.bound(1));
    assert!(host.open(read, "Example.docx".into(), vec![]).is_err());
}

#[test]
fn metadata_limits_are_atomic() {
    let (host, connection) = connected();
    let resource = Resource { id: "one".into(), name: "Example.docx".into(), updated_at: String::new() };
    host.set_resources(&connection, vec![resource.clone()]).unwrap();
    assert!(host.set_resources(&connection, vec![resource.clone(), resource]).is_err());
    assert_eq!(host.snapshot()["resources"].as_array().unwrap().len(), 1);
    assert!(host.choose(1, 1, "../outside.docx".into(), false).is_err());
    assert!(host.connect("bad/id".into(), "Label".into(), "account".into()).is_err());
}

#[test]
fn closed_documents_retire_choices_but_not_acks() {
    let (host, _) = connected();
    let id = request(&host, 1);
    host.publish_documents(json!([]), &[]);
    assert!(host.resolve_dialog(&id, "provider", "").is_err());
    assert!(!host.busy());
}

#[test]
fn disabled_provider_keeps_pending_result_reachable() {
    let (host, _) = connected();
    let id = request(&host, 1);
    host.resolve_dialog(&id, "provider", "").unwrap();
    host.take_commands();
    host.capture(&id, capture(1, 1), vec![]).unwrap();
    host.set_enabled(false).unwrap();
    host.result(id, "cancel".into(), String::new()).unwrap();
    host.take_commands();
    host.disconnect().unwrap();
}

#[test]
fn taking_a_choice_retains_account_and_document_ownership_until_serialization_finishes() {
    let (host, _) = connected();
    let id = request(&host, 1);
    host.resolve_dialog(&id, "device", "").unwrap();
    host.take_commands();
    assert!(host.pending_for(1));
    assert!(host.disconnect().is_err());
    host.finish_device(&id);
    assert!(!host.busy());
    host.disconnect().unwrap();
}

fn fill_event_queue(host: &Host) {
    let mut state = host.state();
    for index in 0..super::MAX_EVENTS {
        state.event(json!({"type":"queued", "index":index}), None).unwrap();
    }
}

#[test]
fn full_event_queue_retains_open_success_and_failure_until_ordered_drain() {
    for success in [true, false] {
        let (host, connection) = connected();
        host.set_resources(&connection, vec![Resource { id: "one".into(), name: "One.zip".into(), updated_at: String::new() }]).unwrap();
        let read = host.request_open("one").unwrap();
        host.open(read.clone(), "One.zip".into(), vec![7]).unwrap();
        host.take_commands();
        host.take_events();
        fill_event_queue(&host);
        host.opened(&read, success.then(|| capture(1, 1)), "decoder failed\n");
        host.opened(&read, None, "duplicate");
        assert!(host.busy());
        assert!(host.disconnect().is_err());
        assert_eq!(host.state().finished.len(), 1);
        let earlier = host.take_events();
        assert_eq!(earlier.len(), super::MAX_EVENTS);
        assert!(earlier.iter().all(|event| event.metadata["type"] == "queued"));
        let terminal = host.take_events();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].metadata["requestId"], read);
        assert_eq!(terminal[0].metadata["resourceId"], "one");
        assert_eq!(terminal[0].metadata["namespace"], "account-one");
        assert_eq!(terminal[0].metadata["connectionId"], connection);
        assert_eq!(terminal[0].metadata["type"], if success { "opened" } else { "openFailed" });
        assert_eq!(host.bound(1), success);
        assert!(!host.busy());
        assert!(host.take_events().is_empty());
        host.disconnect().unwrap();
    }
}

#[test]
fn full_event_queue_retains_save_failure_and_rejects_unowned_or_duplicate_terminals() {
    let (host, _) = connected();
    let save = request(&host, 1);
    host.resolve_dialog(&save, "provider", "").unwrap();
    host.take_commands();
    fill_event_queue(&host);
    host.failed(&save, "encoder failed");
    host.failed(&save, "duplicate");
    host.failed("unowned", "forged");
    assert_eq!(host.state().finished.len(), 1);
    assert!(host.pending_for(1));
    assert!(host.disconnect().is_err());
    for doc in 2..100 {
        assert!(host.choose(doc, 1, "Other.zip".into(), false).is_err());
    }
    assert_eq!(host.take_events().len(), super::MAX_EVENTS);
    let terminal = host.take_events();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].metadata["type"], "saveFailed");
    assert_eq!(terminal[0].metadata["requestId"], save);
    assert_eq!(terminal[0].metadata["error"], "encoder failed");
    assert!(!host.pending_for(1));
    assert!(!host.busy());
    host.failed(&save, "late duplicate");
    assert!(host.take_events().is_empty());
}

#[test]
fn retained_terminals_are_bounded_by_already_admitted_reads_and_saves() {
    let (host, connection) = connected();
    host.set_resources(&connection, vec![Resource { id: "one".into(), name: "One.zip".into(), updated_at: String::new() }]).unwrap();
    let mut saves = Vec::new();
    let mut reads = Vec::new();
    for doc in 1..=super::MAX_PENDING as u64 {
        let save = request(&host, doc);
        host.resolve_dialog(&save, "provider", "").unwrap();
        host.take_commands();
        saves.push(save);
        let read = host.request_open("one").unwrap();
        host.open(read.clone(), "One.zip".into(), vec![]).unwrap();
        host.take_commands();
        host.take_events();
        reads.push(read);
    }
    fill_event_queue(&host);
    for read in &reads {
        host.opened(read, None, "decode failed");
    }
    for save in &saves {
        host.failed(save, "encode failed");
    }
    assert_eq!(host.state().finished.len(), super::MAX_PENDING * 2);
    assert!(host.request_open("one").is_err());
    assert!(host.choose(999, 1, "New.zip".into(), false).is_err());
    assert_eq!(host.take_events().len(), super::MAX_EVENTS);
    let terminals = host.take_events();
    assert_eq!(terminals.len(), super::MAX_PENDING * 2);
    let expected: Vec<_> = reads.iter().chain(&saves).map(String::as_str).collect();
    let delivered: Vec<_> = terminals.iter().map(|event| event.metadata["requestId"].as_str().unwrap()).collect();
    assert_eq!(delivered, expected, "retained completions must preserve completion order");
    assert!(terminals.iter().all(|event| event.metadata["connectionId"] == connection && event.metadata["namespace"] == "account-one"));
    assert!(terminals.len() <= super::MAX_EVENTS);
    assert!(!host.busy());
    assert!(host.take_events().is_empty());
}

#[test]
fn failed_save_delivery_transfers_only_owned_capture_even_with_full_commands() {
    let (host, connection) = connected();
    host.set_resources(&connection, vec![Resource { id: "one".into(), name: "Audio.scraftz".into(), updated_at: String::new() }]).unwrap();
    let save = request(&host, 1);
    host.resolve_dialog(&save, "provider", "Audio.scraftz").unwrap();
    host.take_commands();
    host.capture(&save, Capture { name: "Audio.scraftz".into(), ..capture(1, 23) }, vec![1, 2, 3]).unwrap();
    for _ in 0..super::MAX_PENDING {
        let read = host.request_open("one").unwrap();
        host.open(read, "Audio.scraftz".into(), vec![7]).unwrap();
    }
    let mut failures = 0;
    let mut terminals = 0;
    assert!(host.publish_events_with(
        |metadata, _| {
            if metadata["type"] == "saveFailed" {
                terminals += 1;
                assert_eq!(metadata["requestId"], save);
                assert_eq!(metadata["connectionId"], connection);
            }
            if metadata["type"] == "save" { Err("copy failed") } else { Ok(()) }
        },
        |_| failures += 1,
    ));
    assert_eq!(failures, 1);
    assert_eq!(terminals, 1);
    assert!(host.pending_for(1));
    assert!(!host.bound(1));
    assert!(host.result(save.clone(), "saved".into(), "late".into()).is_err());
    assert_eq!(host.state().commands.len(), super::MAX_PENDING);
    assert_eq!(host.state().delivery_failures.len(), 1);
    let commands = host.take_commands();
    assert_eq!(commands.len(), super::MAX_PENDING + 1, "failed capture transfers already admitted work, never a new command admission");
    assert_eq!(commands.iter().filter(|command| matches!(command, Command::Open { .. })).count(), super::MAX_PENDING);
    let failed = commands
        .iter()
        .filter_map(|command| match command {
            Command::Result { capture, resource, status } => Some((capture, resource, status)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(failed.len(), 1);
    assert_eq!((failed[0].0.doc, failed[0].0.revision, failed[0].0.name.as_str()), (1, 23, "Audio.scraftz"));
    assert!(failed[0].1.is_empty());
    assert_eq!(failed[0].2, "failed");
    assert!(host.state().delivery_failures.is_empty());
}

#[test]
fn accepted_reentrant_save_result_wins_and_schedules_the_actual_consumer() {
    for dispatch_fails in [false, true] {
        let (host, _) = connected();
        let save = request(&host, 1);
        host.resolve_dialog(&save, "provider", "").unwrap();
        host.take_commands();
        host.capture(&save, capture(1, 8), vec![9]).unwrap();
        assert!(
            host.publish_events_with(
                |metadata, _| {
                    assert_eq!(metadata["type"], "save");
                    host.result(save.clone(), "saved".into(), "durable".into()).unwrap();
                    if dispatch_fails { Err("after accepted callback") } else { Ok(()) }
                },
                |_| {},
            ),
            "accepted synchronous results must request a following logic frame"
        );
        assert!(host.pending_for(1));
        assert!(host.state().delivery_failures.is_empty());
        let commands = host.take_commands();
        assert_eq!(commands.len(), 1);
        assert!(matches!(&commands[0], Command::Result { capture, resource, status }
            if capture.doc == 1 && capture.revision == 8 && resource == "durable" && status == "saved"));
        assert!(host.result(save, "failed".into(), String::new()).is_err());
    }
}

#[test]
fn failed_read_is_tombstoned_before_a_full_queue_and_later_same_batch_callback() {
    let (host, connection) = connected();
    host.set_resources(&connection, vec![Resource { id: "one".into(), name: "Audio.scraftz".into(), updated_at: String::new() }]).unwrap();
    let first = host.request_open("one").unwrap();
    let second = host.request_open("one").unwrap();
    let mut attempted_late_reply = false;
    assert!(host.publish_events_with(
        |metadata, _| {
            if metadata["requestId"] == first {
                fill_event_queue(&host);
                return Err("request dispatch failed");
            }
            if metadata["requestId"] == second {
                attempted_late_reply = true;
                assert!(!host.state().reads.contains_key(&first));
                assert!(host.state().opening.contains_key(&first));
                assert!(host.state().finished.iter().any(|terminal| terminal.request == first));
                assert!(host.open(first.clone(), "Late.scraftz".into(), vec![1]).is_err());
                host.open(second.clone(), "Audio.scraftz".into(), vec![2]).unwrap();
                return Err("after accepted open reply");
            }
            Ok(())
        },
        |_| {},
    ));
    assert!(attempted_late_reply);
    let commands = host.take_commands();
    assert_eq!(commands.len(), 1);
    assert!(matches!(&commands[0], Command::Open { request, bytes, .. } if request == &second && bytes == &[2]));
    let terminal = host.take_events();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].metadata["type"], "openFailed");
    assert_eq!(terminal[0].metadata["requestId"], first);
    assert_eq!(terminal[0].metadata["resourceId"], "one");
    assert!(host.open(first, "Late.scraftz".into(), vec![1]).is_err());
}

#[test]
fn failed_dialog_delivery_retires_only_an_unresolved_choice() {
    for accepted in [false, true] {
        let (host, _) = connected();
        let save = request(&host, 1);
        // request() drained its event: use a fresh chooser without draining its publication.
        host.resolve_dialog(&save, "cancel", "").unwrap();
        host.choose(1, 1, "Audio.scraftz".into(), false).unwrap();
        let mut delivered = 0;
        assert!(host.publish_events_with(
            |metadata, _| {
                if metadata["type"] == "saveDialog" {
                    delivered += 1;
                    if accepted {
                        host.resolve_dialog(metadata["requestId"].as_str().unwrap(), "provider", "").unwrap();
                    }
                    return Err("chooser dispatch failed");
                }
                assert_eq!(metadata["type"], "saveFailed");
                Ok(())
            },
            |_| {},
        ));
        assert_eq!(delivered, 1);
        let commands = host.take_commands();
        assert_eq!(commands.len(), usize::from(accepted));
        if accepted {
            assert!(matches!(&commands[0], Command::Save { doc: 1, .. }));
        } else {
            assert!(!host.pending_for(1));
            assert!(!host.busy());
        }
    }
}

#[test]
fn queued_and_inflight_terminals_hold_the_account_and_block_nested_publication() {
    let (host, connection) = connected();
    host.set_resources(&connection, vec![Resource { id: "one".into(), name: "Audio.scraftz".into(), updated_at: String::new() }]).unwrap();
    let read = host.request_open("one").unwrap();
    host.open(read.clone(), "Audio.scraftz".into(), vec![1]).unwrap();
    host.take_commands();
    host.take_events();
    host.opened(&read, None, "decoder failed");
    assert!(host.disconnect().is_err(), "queued terminal still belongs to the account");
    let mut calls = 0;
    host.publish_events_with(
        |metadata, _| {
            calls += 1;
            assert_eq!(metadata["type"], "openFailed");
            assert_eq!(metadata["connectionId"], connection);
            assert!(host.disconnect().is_err(), "last drained terminal remains owned during its callback");
            assert!(!host.publish_events_with(|_, _| Ok::<(), ()>(()), |_| {}));
            Ok::<(), ()>(())
        },
        |_| {},
    );
    assert_eq!(calls, 1);
    assert!(!host.busy());
    host.disconnect().unwrap();
}

#[test]
fn full_command_queue_keeps_a_choice_until_retry_or_explicit_cancel() {
    let (host, connection) = connected();
    let save = request(&host, 1);
    host.set_resources(&connection, vec![Resource { id: "one".into(), name: "Audio.scraftz".into(), updated_at: String::new() }]).unwrap();
    for _ in 0..super::MAX_PENDING {
        let read = host.request_open("one").unwrap();
        host.open(read, "Audio.scraftz".into(), vec![1]).unwrap();
    }
    assert!(host.resolve_dialog(&save, "provider", "").is_err());
    assert!(host.state().dialogs.contains_key(&save));
    assert_eq!(host.state().commands.len(), super::MAX_PENDING);
    host.resolve_dialog(&save, "cancel", "").unwrap();
    assert!(!host.state().dialogs.contains_key(&save));
    assert_eq!(host.take_commands().len(), super::MAX_PENDING);
}
