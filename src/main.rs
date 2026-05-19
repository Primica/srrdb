mod repl;
mod sql;

fn main() {
    let mut repl = repl::Repl::new();
    repl.run();
}
