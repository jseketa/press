//! CSS plus computation. lace compiles a small language into CSS: CSS
//! itself, plus variables, functions, mixins, conditionals and loops.
//! Anything CSS can do on its own is passed through untouched. SPEC.md is
//! the language reference.

mod ast;
mod builtins;
mod eval;
mod parse;
mod source;
mod value;

use std::path::Path;

pub use eval::{Loader, Log, DEFAULT_BUDGET};
pub use source::Error;

const PRELUDE: &str = include_str!("../prelude.scss");

/// The spec allows 500 levels of syntactic nesting and 1,000 nested calls;
/// a compile runs on its own thread with room for that, reserved rather
/// than used.
const STACK_SIZE: usize = 256 << 20;

/// What a compilation needs from its host: how to read files, where @debug
/// output goes, and how much work to allow.
pub struct Compiler {
    load: Loader,
    log: Log,
    budget: i64,
}

impl Default for Compiler {
    fn default() -> Compiler {
        Compiler::new()
    }
}

impl Compiler {
    /// Reads from disk, logs to standard error, allows ten million steps.
    pub fn new() -> Compiler {
        Compiler {
            load: Box::new(|path| std::fs::read_to_string(path).map_err(|e| e.to_string())),
            log: Box::new(std::io::stderr()),
            budget: DEFAULT_BUDGET,
        }
    }

    /// How @use paths are read; they arrive resolved, with forward slashes.
    pub fn load(mut self, load: Loader) -> Compiler {
        self.load = load;
        self
    }

    /// Where @debug lines go.
    pub fn log(mut self, log: Log) -> Compiler {
        self.log = log;
        self
    }

    /// Evaluation steps allowed; 0 or negative for unlimited.
    pub fn budget(mut self, budget: i64) -> Compiler {
        self.budget = budget;
        self
    }

    /// Compiles src, named name in errors and as the anchor for relative
    /// @use paths, and returns the CSS.
    pub fn compile(self, src: &str, name: &str) -> Result<String, Error> {
        let (src, file) = (src.to_string(), name.to_string());
        let internal = |msg: String| Error { file: name.to_string(), line: 0, col: 0, msg, frames: Vec::new() };
        let worker = std::thread::Builder::new()
            .name("lace".into())
            .stack_size(STACK_SIZE)
            .spawn(move || self.run(&src, &file))
            .map_err(|e| internal(format!("cannot start compile thread: {e}")))?;
        worker.join().unwrap_or_else(|_| Err(internal("internal error (please report the stylesheet)".into())))
    }

    fn run(self, src: &str, name: &str) -> Result<String, Error> {
        let mut ev = eval::Evaluator::new(self.load, self.log, self.budget);
        let prelude = source::Source::new("prelude", PRELUDE);
        ev.run(&parse::parse(&prelude)?, "prelude")?;
        ev.lock_functions();
        ev.reset_steps(); // the budget is the stylesheet's, not the prelude's
        let main = source::Source::new(name, src);
        ev.run(&parse::parse(&main)?, name)?;
        Ok(ev.render())
    }
}

/// Compiles a file from disk.
pub fn compile_file(path: impl AsRef<Path>) -> Result<String, Error> {
    let path = path.as_ref();
    let name = path.to_string_lossy().replace('\\', "/");
    let src = std::fs::read_to_string(path).map_err(|e| Error {
        file: name.clone(),
        line: 0,
        col: 0,
        msg: e.to_string(),
        frames: Vec::new(),
    })?;
    Compiler::new().compile(&src, &name)
}

/// Compiles source held in memory with the default loader and log.
pub fn compile(src: &str, name: &str) -> Result<String, Error> {
    Compiler::new().compile(src, name)
}

/// The source of the standard library.
pub fn prelude() -> &'static str {
    PRELUDE
}
