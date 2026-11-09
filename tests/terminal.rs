use myopc::input::{command_input_guard, input_is_busy, prompt_guard};
use myopc::notification::NotificationQueue;

#[test]
fn prompts_exclude_command_input_until_released() {
    let prompt = prompt_guard();
    assert!(input_is_busy());
    assert!(command_input_guard().is_none());
    drop(prompt);

    assert!(!input_is_busy());
    let command = command_input_guard().unwrap();
    assert!(input_is_busy());
    assert!(command_input_guard().is_none());
    drop(command);
    assert!(!input_is_busy());
}

#[test]
fn notifications_are_fifo_and_drained_atomically() {
    let queue = NotificationQueue::default();
    queue.push("first");
    queue.push("second");

    assert_eq!(queue.drain(), vec!["first", "second"]);
    assert!(queue.drain().is_empty());
}
