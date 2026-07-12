use std::{
    io,
    sync::{Mutex, MutexGuard, Once},
    thread::{self, ThreadId},
};

#[derive(Debug, Default)]
struct TerminalPanicRestoreOwnerState {
    owner: Option<ThreadId>,
    claim_count: usize,
}

/// `TerminalPanicRestoreRegistry` 记录当前有权执行 emergency restore 的线程。
#[derive(Debug)]
pub(crate) struct TerminalPanicRestoreRegistry {
    state: Mutex<TerminalPanicRestoreOwnerState>,
}

impl TerminalPanicRestoreRegistry {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(TerminalPanicRestoreOwnerState {
                owner: None,
                claim_count: 0,
            }),
        }
    }

    pub(crate) fn claim_current_thread(&'static self) -> io::Result<TerminalPanicRestoreClaim> {
        let current_thread = thread::current().id();
        let mut state = self.lock_state();
        match state.owner {
            None => {
                state.owner = Some(current_thread);
                state.claim_count = 1;
            }
            Some(owner) if owner == current_thread => {
                state.claim_count = state
                    .claim_count
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("terminal lifecycle claim count overflowed"))?;
            }
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "terminal lifecycle is already owned by another thread",
                ));
            }
        }
        drop(state);

        Ok(TerminalPanicRestoreClaim {
            registry: self,
            owner: current_thread,
        })
    }

    pub(crate) fn is_current_thread_owner(&self) -> bool {
        self.lock_state().owner == Some(thread::current().id())
    }

    fn lock_state(&self) -> MutexGuard<'_, TerminalPanicRestoreOwnerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn release(&self, owner: ThreadId) {
        let mut state = self.lock_state();
        if state.owner != Some(owner) || state.claim_count == 0 {
            return;
        }

        state.claim_count -= 1;
        if state.claim_count == 0 {
            state.owner = None;
        }
    }
}

/// RAII claim，确保最后一个 terminal lifecycle owner 退出时清除 registry。
#[derive(Debug)]
pub(crate) struct TerminalPanicRestoreClaim {
    registry: &'static TerminalPanicRestoreRegistry,
    owner: ThreadId,
}

impl Drop for TerminalPanicRestoreClaim {
    fn drop(&mut self) {
        self.registry.release(self.owner);
    }
}

static TERMINAL_PANIC_RESTORE_REGISTRY: TerminalPanicRestoreRegistry =
    TerminalPanicRestoreRegistry::new();

pub(crate) fn terminal_panic_restore_registry() -> &'static TerminalPanicRestoreRegistry {
    &TERMINAL_PANIC_RESTORE_REGISTRY
}

struct TerminalPanicHookInstaller {
    once: Once,
}

impl TerminalPanicHookInstaller {
    const fn new() -> Self {
        Self { once: Once::new() }
    }

    fn install(&self, install: impl FnOnce()) {
        self.once.call_once(install);
    }
}

static INSTALL_TERMINAL_PANIC_HOOK: TerminalPanicHookInstaller = TerminalPanicHookInstaller::new();

fn run_terminal_panic_hook(
    has_restore_authority: bool,
    restore: impl FnOnce() -> io::Result<()>,
    previous_hook: impl FnOnce(),
) {
    if has_restore_authority {
        let _ = restore();
    }
    previous_hook();
}

/// 安装进程级 terminal panic hook。
///
/// 首次调用会包装既有 panic report hook；后续调用无操作。调用方应先安装
/// `color_eyre` 等报告 hook，再调用本函数。
pub fn install_terminal_panic_hook() {
    INSTALL_TERMINAL_PANIC_HOOK.install(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            run_terminal_panic_hook(
                terminal_panic_restore_registry().is_current_thread_owner(),
                super::terminal_lifecycle::restore_terminal_after_panic,
                || previous_hook(panic_info),
            );
        }));
    });
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, cell::RefCell, io, thread};

    use super::{
        TerminalPanicHookInstaller, TerminalPanicRestoreRegistry, run_terminal_panic_hook,
    };

    #[test]
    fn same_thread_claims_nest_until_the_last_claim_drops() {
        let registry = Box::leak(Box::new(TerminalPanicRestoreRegistry::new()));
        let first = registry
            .claim_current_thread()
            .expect("first lifecycle claim should succeed");
        let second = registry
            .claim_current_thread()
            .expect("same-thread nested lifecycle claim should succeed");

        assert!(registry.is_current_thread_owner());
        drop(first);
        assert!(registry.is_current_thread_owner());
        drop(second);
        assert!(!registry.is_current_thread_owner());
    }

    #[test]
    fn another_thread_cannot_claim_an_active_terminal_lifecycle() {
        let registry = Box::leak(Box::new(TerminalPanicRestoreRegistry::new()));
        let other_thread_registry: &'static TerminalPanicRestoreRegistry = registry;
        let owner_claim = registry
            .claim_current_thread()
            .expect("first lifecycle claim should succeed");

        let error = thread::spawn(move || {
            other_thread_registry
                .claim_current_thread()
                .expect_err("another thread must not share terminal lifecycle ownership")
        })
        .join()
        .expect("claim test thread should finish");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        drop(owner_claim);
    }

    #[test]
    fn active_owner_restores_before_reporting_the_panic() {
        let calls = RefCell::new(Vec::new());

        run_terminal_panic_hook(
            true,
            || {
                calls.borrow_mut().push("restore");
                Ok(())
            },
            || calls.borrow_mut().push("previous_hook"),
        );

        assert_eq!(*calls.borrow(), ["restore", "previous_hook"]);
    }

    #[test]
    fn unauthorized_panic_only_reports() {
        let calls = RefCell::new(Vec::new());

        run_terminal_panic_hook(
            false,
            || {
                calls.borrow_mut().push("restore");
                Ok(())
            },
            || calls.borrow_mut().push("previous_hook"),
        );

        assert_eq!(*calls.borrow(), ["previous_hook"]);
    }

    #[test]
    fn restore_failure_still_reports_the_panic() {
        let calls = RefCell::new(Vec::new());

        run_terminal_panic_hook(
            true,
            || {
                calls.borrow_mut().push("restore");
                Err(io::Error::other("restore failed"))
            },
            || calls.borrow_mut().push("previous_hook"),
        );

        assert_eq!(*calls.borrow(), ["restore", "previous_hook"]);
    }

    #[test]
    fn hook_installer_runs_installation_once() {
        let installer = TerminalPanicHookInstaller::new();
        let installations = Cell::new(0);

        installer.install(|| installations.set(installations.get() + 1));
        installer.install(|| installations.set(installations.get() + 1));

        assert_eq!(installations.get(), 1);
    }
}
