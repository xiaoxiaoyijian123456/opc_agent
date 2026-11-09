use crate::app::MYOPCApp;
use crate::input::{command_input_guard, input_is_busy};
use rustyline::error::ReadlineError;
use tokio::task::JoinHandle;

pub async fn run(app: MYOPCApp) {
    let mut readline = match rustyline::DefaultEditor::new() {
        Ok(readline) => readline,
        Err(error) => {
            eprintln!("failed to init readline: {error}");
            return;
        }
    };

    let mut background_chat: Option<JoinHandle<()>> = None;
    loop {
        if background_chat
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            if let Some(task) = background_chat.take() {
                let _ = task.await;
            }
        }
        if !input_is_busy() {
            if let Some(output_guard) = command_input_guard() {
                for message in app.drain_notifications() {
                    println!("{message}");
                }
                drop(output_guard);
            }
        }
        if input_is_busy() {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            continue;
        }
        let Some(input_guard) = command_input_guard() else {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            continue;
        };
        match readline.readline("myopc > ") {
            Ok(line) => {
                drop(input_guard);
                readline.add_history_entry(line.as_str()).ok();
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.starts_with('/') {
                    if app.handle_slash(trimmed).await {
                        break;
                    }
                } else {
                    if background_chat
                        .as_ref()
                        .is_some_and(|task| !task.is_finished())
                    {
                        eprintln!("A chat is still running. Use /status, /tasks, or /exit while it completes.");
                        continue;
                    }
                    if let Some(task) = background_chat.take() {
                        let _ = task.await;
                    }
                    let background_app = app.clone();
                    let input = trimmed.to_string();
                    background_chat = Some(tokio::spawn(async move {
                        background_app.chat(&input).await;
                    }));
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => {
                drop(input_guard);
                break;
            }
            Err(error) => {
                drop(input_guard);
                eprintln!("{error}");
                break;
            }
        }
    }
    if let Some(task) = background_chat {
        task.abort();
    }
    app.shutdown();
    println!("\nBye.");
}
