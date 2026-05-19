use std::path::PathBuf;

use colored::Colorize;
use rustyline::config::Builder as RlConfigBuilder;
use rustyline::error::ReadlineError;
use rustyline::history::{FileHistory, History};
use rustyline::Editor;

use crate::sql;

const HISTORY_FILE: &str = ".srrdb_history";
const PROMPT: &str = "srrdb> ";
const PROMPT_CONT: &str = "  ...> ";

pub struct Repl {
    rl: Editor<(), FileHistory>,
    history_path: PathBuf,
    buffer: String,
}

impl Repl {
    pub fn new() -> Self {
        let config = RlConfigBuilder::new()
            .build();

        let mut rl = Editor::with_config(config).expect("failed to create REPL editor");
        let _ = rl.history_mut().set_max_len(1000);

        let history_path = PathBuf::from(HISTORY_FILE);
        if history_path.exists() {
            let _ = rl.load_history(&history_path);
        }

        Self {
            rl,
            history_path,
            buffer: String::new(),
        }
    }

    pub fn run(&mut self) {
        println!("{}", "srrdb REPL — type SQL or .help for commands".cyan());
        println!("{}", "Ctrl+D or .exit to quit".dimmed());

        loop {
            let prompt = if self.buffer.is_empty() { PROMPT } else { PROMPT_CONT };

            let line = match self.rl.readline(prompt) {
                Ok(line) => line,
                Err(ReadlineError::Interrupted) => {
                    if !self.buffer.is_empty() {
                        self.buffer.clear();
                        println!("{}", "^C (buffer cleared)".yellow());
                        continue;
                    }
                    println!("{}", "^C".yellow());
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!();
                    break;
                }
                Err(e) => {
                    eprintln!("{}: {e}", "REPL error".red());
                    break;
                }
            };

            let trimmed = line.trim().to_string();

            if trimmed.is_empty() && self.buffer.is_empty() {
                continue;
            }

            if trimmed.starts_with('.') {
                if !self.buffer.is_empty() {
                    self.buffer.clear();
                }
                if !self.handle_command(&trimmed) {
                    break;
                }
                continue;
            }

            self.buffer.push_str(&trimmed);
            self.buffer.push(' ');

            if trimmed.ends_with(';') || trimmed.is_empty() {
                let input = self.buffer.trim().to_string();
                self.buffer.clear();

                if !input.is_empty() && input != ";" {
                    let _ = self.rl.add_history_entry(&input);
                    self.eval(&input);
                }
            }
        }

        let _ = self.rl.save_history(&self.history_path);
    }

    fn eval(&self, input: &str) {
        match sql::Parser::parse_sql(input) {
            Ok(statements) => {
                if statements.is_empty() {
                    println!("{}", "No statements parsed".dimmed());
                    return;
                }
                for (i, stmt) in statements.iter().enumerate() {
                    if i > 0 {
                        println!();
                    }
                    println!("{stmt:#?}");
                }
            }
            Err(e) => {
                eprintln!("{} {e}", "Error:".red().bold());
            }
        }
    }

    fn handle_command(&self, cmd: &str) -> bool {
        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        let command = parts[0];
        let args = parts.get(1).copied().unwrap_or("");

        match command {
            ".exit" | ".quit" => false,
            ".help" => {
                println!("{}", "Commands:".green().bold());
                println!("  {:<20}  {}", ".help".cyan(), "Show this help");
                println!("  {:<20}  {}", ".exit / .quit".cyan(), "Exit the REPL");
                println!("  {:<20}  {}", ".tokens <sql>".cyan(), "Tokenize SQL string");
                println!("  {:<20}  {}", ".ast <sql>".cyan(), "Parse and show AST");
                println!();
                println!("{}", "Usage:".green().bold());
                println!("  Type SQL directly (end with ;) to see the parsed AST");
                println!("  Multi-line input: keep typing until you end with ;");
                println!("  Ctrl+C: cancel current input  |  Ctrl+D: exit");
                true
            }
            ".tokens" => {
                if args.is_empty() {
                    eprintln!("{} Usage: .tokens <sql>", "Error:".red().bold());
                    return true;
                }
                match sql::Tokenizer::new(args).tokenize() {
                    Ok(tokens) => {
                        for token in &tokens {
                            println!("  {token:?}");
                        }
                    }
                    Err(e) => {
                        eprintln!("{} {e}", "Error:".red().bold());
                    }
                }
                true
            }
            ".ast" => {
                if args.is_empty() {
                    eprintln!("{} Usage: .ast <sql>", "Error:".red().bold());
                    return true;
                }
                self.eval(args);
                true
            }
            _ => {
                eprintln!("{} Unknown command: {cmd}. Type .help for available commands", "Error:".red().bold());
                true
            }
        }
    }
}
