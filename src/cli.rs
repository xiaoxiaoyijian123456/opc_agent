use crate::app::MYOPCApp;
use rustyline::error::ReadlineError;

pub async fn run(app: MYOPCApp) {
    let mut readline = match rustyline::DefaultEditor::new() {
        Ok(readline) => readline,
        Err(error) => {
            eprintln!("failed to init readline: {error}");
            return;
        }
    };

    loop {
        match readline.readline("myopc > ") {
            Ok(line) => {
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
                    app.chat(trimmed).await;
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(error) => {
                eprintln!("{error}");
                break;
            }
        }
    }
    app.shutdown();
    println!("\nBye.");
}
