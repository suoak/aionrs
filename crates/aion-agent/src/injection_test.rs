use super::*;

#[test]
fn cloned_handles_share_a_fifo_mailbox() {
    let handle = InjectionHandle::default();
    let clone = handle.clone();
    handle.enqueue("one".into(), "first".into());
    clone.enqueue("two".into(), "second".into());

    let drained = handle.drain();
    assert_eq!(
        drained.iter().map(|item| item.input_id.as_str()).collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert!(clone.drain().is_empty());
}
