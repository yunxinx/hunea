use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use conversation_runtime::{NotifyingSender, RuntimeEventNotifier};

#[test]
fn notifier_is_silent_until_a_callback_is_installed() {
    let notifier = RuntimeEventNotifier::default();

    notifier.notify();
}

#[test]
fn notifier_invokes_the_installed_callback() {
    let wake_count = Arc::new(AtomicUsize::new(0));
    let callback_wake_count = Arc::clone(&wake_count);
    let notifier = RuntimeEventNotifier::default();
    notifier
        .install(move || {
            callback_wake_count.fetch_add(1, Ordering::SeqCst);
        })
        .expect("first callback installation should succeed");

    notifier.notify();
    notifier.notify();

    assert_eq!(wake_count.load(Ordering::SeqCst), 2);
}

#[test]
fn notifier_rejects_replacing_an_installed_callback() {
    let notifier = RuntimeEventNotifier::default();
    notifier
        .install(|| {})
        .expect("first callback installation should succeed");

    let error = notifier
        .install(|| {})
        .expect_err("second callback installation should be rejected");

    assert_eq!(
        error.to_string(),
        "runtime event notifier callback is already installed"
    );
}

#[test]
fn notifier_guard_invokes_callback_when_the_worker_scope_exits() {
    let wake_count = Arc::new(AtomicUsize::new(0));
    let callback_wake_count = Arc::clone(&wake_count);
    let notifier = RuntimeEventNotifier::default();
    notifier
        .install(move || {
            callback_wake_count.fetch_add(1, Ordering::SeqCst);
        })
        .expect("callback installation should succeed");

    {
        let _exit_notification = notifier.notify_on_drop();
    }

    assert_eq!(wake_count.load(Ordering::SeqCst), 1);
}

#[test]
fn notifying_sender_makes_payload_visible_before_notification() {
    let notifier = RuntimeEventNotifier::default();
    let (sender, receiver) = std::sync::mpsc::channel();
    let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
    let callback_receiver = std::sync::Arc::clone(&receiver);
    notifier
        .install(move || {
            assert_eq!(callback_receiver.lock().unwrap().try_recv(), Ok("payload"));
        })
        .unwrap();

    NotifyingSender::new(sender, notifier)
        .send("payload")
        .unwrap();
}

#[test]
fn notifying_sender_does_not_notify_when_payload_send_fails() {
    let notifier = RuntimeEventNotifier::default();
    let notifications = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_notifications = std::sync::Arc::clone(&notifications);
    notifier
        .install(move || {
            callback_notifications.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
    let (sender, receiver) = std::sync::mpsc::channel::<()>();
    drop(receiver);

    assert!(NotifyingSender::new(sender, notifier).send(()).is_err());
    assert_eq!(notifications.load(std::sync::atomic::Ordering::SeqCst), 0);
}
