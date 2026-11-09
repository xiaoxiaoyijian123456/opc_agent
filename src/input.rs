use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};

struct InputState {
    gate: Mutex<()>,
    pending_prompts: AtomicUsize,
    active: AtomicBool,
}

static INPUT: OnceLock<InputState> = OnceLock::new();

fn state() -> &'static InputState {
    INPUT.get_or_init(|| InputState {
        gate: Mutex::new(()),
        pending_prompts: AtomicUsize::new(0),
        active: AtomicBool::new(false),
    })
}

pub struct PromptGuard {
    state: &'static InputState,
    _guard: MutexGuard<'static, ()>,
}

impl Drop for PromptGuard {
    fn drop(&mut self) {
        self.state.active.store(false, Ordering::SeqCst);
        self.state.pending_prompts.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct CommandInputGuard {
    state: &'static InputState,
    _guard: MutexGuard<'static, ()>,
}

impl Drop for CommandInputGuard {
    fn drop(&mut self) {
        self.state.active.store(false, Ordering::SeqCst);
    }
}

pub fn prompt_guard() -> PromptGuard {
    let state = state();
    state.pending_prompts.fetch_add(1, Ordering::SeqCst);
    let guard = state
        .gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.active.store(true, Ordering::SeqCst);
    PromptGuard {
        state,
        _guard: guard,
    }
}

pub fn command_input_guard() -> Option<CommandInputGuard> {
    let state = state();
    if input_is_busy() {
        return None;
    }
    let guard = match state.gate.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(TryLockError::WouldBlock) => return None,
    };
    if state.pending_prompts.load(Ordering::SeqCst) > 0 {
        return None;
    }
    state.active.store(true, Ordering::SeqCst);
    Some(CommandInputGuard {
        state,
        _guard: guard,
    })
}

pub fn input_is_busy() -> bool {
    let state = state();
    state.pending_prompts.load(Ordering::SeqCst) > 0 || state.active.load(Ordering::SeqCst)
}
