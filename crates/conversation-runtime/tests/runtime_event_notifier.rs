use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use conversation_runtime::{NotifyingSender, RuntimeEventNotifier};

#[test]
fn notifier_is_silent_until_a_callback_is_bound() {
    let notifier = RuntimeEventNotifier::default();

    notifier.notify();
}

#[test]
fn notifier_invokes_the_current_callback() {
    let wake_count = Arc::new(AtomicUsize::new(0));
    let callback_wake_count = Arc::clone(&wake_count);
    let notifier = RuntimeEventNotifier::default();
    notifier.replace_callback(move || {
        callback_wake_count.fetch_add(1, Ordering::SeqCst);
    });

    notifier.notify();
    notifier.notify();

    assert_eq!(wake_count.load(Ordering::SeqCst), 2);
}

#[test]
fn notifier_replaces_the_callback_for_subsequent_notifications() {
    let old_wake_count = Arc::new(AtomicUsize::new(0));
    let old_callback_wake_count = Arc::clone(&old_wake_count);
    let new_wake_count = Arc::new(AtomicUsize::new(0));
    let new_callback_wake_count = Arc::clone(&new_wake_count);
    let notifier = RuntimeEventNotifier::default();

    notifier.replace_callback(move || {
        old_callback_wake_count.fetch_add(1, Ordering::SeqCst);
    });
    notifier.notify();
    notifier.replace_callback(move || {
        new_callback_wake_count.fetch_add(1, Ordering::SeqCst);
    });
    notifier.notify();

    assert_eq!(old_wake_count.load(Ordering::SeqCst), 1);
    assert_eq!(new_wake_count.load(Ordering::SeqCst), 1);
}

#[test]
fn notifier_clones_share_the_latest_callback() {
    let wake_count = Arc::new(AtomicUsize::new(0));
    let callback_wake_count = Arc::clone(&wake_count);
    let notifier = RuntimeEventNotifier::default();
    let producer_notifier = notifier.clone();

    notifier.replace_callback(move || {
        callback_wake_count.fetch_add(1, Ordering::SeqCst);
    });
    producer_notifier.notify();

    assert_eq!(wake_count.load(Ordering::SeqCst), 1);
}

#[test]
fn callback_can_replace_itself_without_deadlocking() {
    let initial_wake_count = Arc::new(AtomicUsize::new(0));
    let initial_callback_wake_count = Arc::clone(&initial_wake_count);
    let replacement_wake_count = Arc::new(AtomicUsize::new(0));
    let replacement_callback_wake_count = Arc::clone(&replacement_wake_count);
    let notifier = RuntimeEventNotifier::default();
    let callback_notifier = notifier.clone();

    notifier.replace_callback(move || {
        initial_callback_wake_count.fetch_add(1, Ordering::SeqCst);
        let replacement_callback_wake_count = Arc::clone(&replacement_callback_wake_count);
        callback_notifier.replace_callback(move || {
            replacement_callback_wake_count.fetch_add(1, Ordering::SeqCst);
        });
    });

    notifier.notify();
    notifier.notify();

    assert_eq!(initial_wake_count.load(Ordering::SeqCst), 1);
    assert_eq!(replacement_wake_count.load(Ordering::SeqCst), 1);
}

#[test]
fn notifier_guard_invokes_the_latest_callback_when_the_worker_scope_exits() {
    let old_wake_count = Arc::new(AtomicUsize::new(0));
    let old_callback_wake_count = Arc::clone(&old_wake_count);
    let new_wake_count = Arc::new(AtomicUsize::new(0));
    let new_callback_wake_count = Arc::clone(&new_wake_count);
    let notifier = RuntimeEventNotifier::default();

    notifier.replace_callback(move || {
        old_callback_wake_count.fetch_add(1, Ordering::SeqCst);
    });
    let exit_notification = notifier.notify_on_drop();
    notifier.replace_callback(move || {
        new_callback_wake_count.fetch_add(1, Ordering::SeqCst);
    });
    drop(exit_notification);

    assert_eq!(old_wake_count.load(Ordering::SeqCst), 0);
    assert_eq!(new_wake_count.load(Ordering::SeqCst), 1);
}

#[test]
fn notifying_sender_makes_payload_visible_before_notification() {
    let notifier = RuntimeEventNotifier::default();
    let (sender, receiver) = std::sync::mpsc::channel();
    let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
    let callback_receiver = std::sync::Arc::clone(&receiver);
    notifier.replace_callback(move || {
        assert_eq!(callback_receiver.lock().unwrap().try_recv(), Ok("payload"));
    });

    NotifyingSender::new(sender, notifier)
        .send("payload")
        .unwrap();
}

#[test]
fn notifying_sender_does_not_notify_when_payload_send_fails() {
    let notifier = RuntimeEventNotifier::default();
    let notifications = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_notifications = std::sync::Arc::clone(&notifications);
    notifier.replace_callback(move || {
        callback_notifications.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let (sender, receiver) = std::sync::mpsc::channel::<()>();
    drop(receiver);

    assert!(NotifyingSender::new(sender, notifier).send(()).is_err());
    assert_eq!(notifications.load(std::sync::atomic::Ordering::SeqCst), 0);
}
