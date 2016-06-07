extern crate regex;

pub use super::super::compiler::*;

use super::postprocess;
use super::super::utils::filter;
use super::super::io::memstream::MemStream;
use super::super::io::tempfile::TempFile;
use super::super::lazy::Lazy;

use std::fs::File;
use std::io::{Error, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use self::regex::bytes::{Regex, NoExpand};

pub struct VsCompiler {
	temp_dir: PathBuf,
	toolchains: ToolchainHolder,
}

impl VsCompiler {
	pub fn new(temp_dir: &Path) -> Self {
		VsCompiler {
			temp_dir: temp_dir.to_path_buf(),
			toolchains: ToolchainHolder::new(),
		}
	}
}

struct VsToolchain {
	temp_dir: PathBuf,
	path: PathBuf,
	identifier: Lazy<Option<String>>,
}

impl VsToolchain {
	pub fn new(path: PathBuf, temp_dir: PathBuf) -> Self {
		VsToolchain {
			temp_dir: temp_dir,
			path: path,
			identifier: Lazy::new(),
		}
	}
}

impl Compiler for VsCompiler {
	fn resolve_toolchain(&self, command: &CommandInfo) -> Option<Arc<Toolchain>> {
		self.toolchains.resolve(command, |path| Arc::new(VsToolchain::new(path, self.temp_dir.clone())))
	}

	fn create_task(&self, command: CommandInfo, args: &[String]) -> Result<Option<CompilationTask>, String> {
		self.resolve_toolchain(&command)
		.ok_or(format!("Can't get toolchain for {}", command.program.display()))
		.and_then(|toolchain| super::prepare::create_task(toolchain, command, args))
	}

	fn preprocess_step(&self, task: &CompilationTask) -> Result<PreprocessResult, Error> {
		// Make parameters list for preprocessing.
		let mut args = filter(&task.args, |arg: &Arg| -> Option<String> {
			match arg {
				&Arg::Flag { ref scope, ref flag } => {
					match scope {
						&Scope::Preprocessor | &Scope::Shared => Some("/".to_string() + &flag),
						&Scope::Ignore | &Scope::Compiler => None
					}
				}
				&Arg::Param { ref scope, ref flag, ref value } => {
					match scope {
						&Scope::Preprocessor | &Scope::Shared => Some("/".to_string() + &flag + &value),
						&Scope::Ignore | &Scope::Compiler => None
					}
				}
				&Arg::Input { .. } => None,
				&Arg::Output { .. } => None,
			}
		});

		// Add preprocessor paramters.
		args.push("/nologo".to_string());
		args.push("/T".to_string() + &task.language);
		args.push("/E".to_string());
		args.push("/we4002".to_string()); // C4002: too many actual parameters for macro 'identifier'
		args.push(task.input_source.display().to_string());

		let mut command = task.command.to_command();
		command
		.args(&args)
		.arg(&join_flag("/Fo", &task.output_object)); // /Fo option also set output path for #import directive
		let output = try!(command.output());
		if output.status.success() {
			let mut content = MemStream::new();
			if task.input_precompiled.is_some() || task.output_precompiled.is_some() {
				try!(postprocess::filter_preprocessed(&mut Cursor::new(output.stdout), &mut content, &task.marker_precompiled, task.output_precompiled.is_some()));
			} else {
				try!(content.write(&output.stdout));
			};
			Ok(PreprocessResult::Success(content))
		} else {
			Ok(PreprocessResult::Failed(OutputInfo {
				status: output.status.code(),
				stdout: Vec::new(),
				stderr: output.stderr,
			}))
		}
	}

	// Compile preprocessed file.
	fn compile_prepare_step(&self, task: CompilationTask, preprocessed: MemStream) -> Result<CompileStep, Error> {
		let mut args = filter(&task.args, |arg: &Arg| -> Option<String> {
			match arg {
				&Arg::Flag { ref scope, ref flag } => {
					match scope {
						&Scope::Compiler | &Scope::Shared => Some("/".to_string() + &flag),
						&Scope::Preprocessor if task.output_precompiled.is_some() => Some("/".to_string() + &flag),
						&Scope::Ignore | &Scope::Preprocessor => None
					}
				}
				&Arg::Param { ref scope, ref flag, ref value } => {
					match scope {
						&Scope::Compiler | &Scope::Shared => Some("/".to_string() + &flag + &value),
						&Scope::Preprocessor if task.output_precompiled.is_some() => Some("/".to_string() + &flag + &value),
						&Scope::Ignore | &Scope::Preprocessor => None
					}
				}
				&Arg::Input { .. } => None,
				&Arg::Output { .. } => None
			}
		});
		args.push("/nologo".to_string());
		args.push("/T".to_string() + &task.language);
		match &task.input_precompiled {
			&Some(ref path) => {
				args.push("/Yu".to_string());
				args.push("/Fp".to_string() + &path.display().to_string());
			}
			&None => {}
		}
		if task.output_precompiled.is_some() {
			args.push("/Yc".to_string());
		}
		Ok(CompileStep::new(task, preprocessed, args, true))
	}
}

impl Toolchain for VsToolchain {
	fn identifier(&self) -> Option<String> {
		self.identifier.get(|| vs_identifier(&self.path))
	}

	fn compile_step(&self, task: CompileStep) -> Result<OutputInfo, Error> {
		// Input file path.
		let input_temp = TempFile::new_in(&self.temp_dir, ".i");
		try! (File::create(input_temp.path()).and_then(|mut s| task.preprocessed.copy(&mut s)));
		// Run compiler.
		let mut command = task.command.to_command();
		command
		.arg("/c")
		.args(&task.args)
		.arg(input_temp.path().to_str().unwrap())
		.arg(&join_flag("/Fo", &task.output_object));
		// Output files.
		match &task.output_precompiled {
			&Some(ref path) => { command.arg(join_flag("/Fp", path)); }
			&None => {}
		}
		match &task.input_precompiled {
			&Some(ref path) => { command.arg(join_flag("/Fp", path)); }
			&None => {}
		}
		// Save input file name for output filter.
		let temp_file = input_temp.path().file_name()
		.and_then(|o| o.to_str())
		.map(|o| o.as_bytes())
		.unwrap_or(b"");
		// Execute.
		command.output().map(|o| OutputInfo {
			status: o.status.code(),
			stdout: prepare_output(temp_file, o.stdout, o.status.code() == Some(0)),
			stderr: o.stderr,
		})
	}
}

fn vs_identifier(clang: &Path) -> Option<String> {
	panic!("TODO: Not implemented yet")
}

fn prepare_output(line: &[u8], mut buffer: Vec<u8>, success: bool) -> Vec<u8> {
	// Remove strage file name from output
	let mut begin = match (line.len() < buffer.len()) && buffer.starts_with(line) && is_eol(buffer[line.len()]) {
		true => line.len(),
		false => 0
	};
	while begin < buffer.len() && is_eol(buffer[begin]) {
		begin += 1;
	}
	buffer = buffer.split_off(begin);
	if success {
		// Remove some redundant lines
		lazy_static! {
			static ref RE: Regex = Regex::new(r"(?m)^\S+[^:]*\(\d+\) : warning C4628: .*$\n?").unwrap();
		}
		buffer = RE.replace_all(&buffer, NoExpand(b""))
	}
	buffer
}

fn is_eol(c: u8) -> bool {
	match c {
	    b'\r' | b'\n' => true,
	    _ => false,
	}
}

fn join_flag(flag: &str, path: &Path) -> String {
	flag.to_string() + &path.to_str().unwrap()
}


#[cfg(test)]
mod test {
    use std::io::Write;

    fn check_prepare_output(original: &str, expected: &str, line: &str, success: bool) {
        let mut stream: Vec<u8> = Vec::new();
        stream.write(&original.as_bytes()[..]).unwrap();

        let result = super::prepare_output(line.as_bytes(), stream, success);
        assert_eq!(String::from_utf8_lossy(&result), expected);
    }

    #[test]
    fn test_prepare_output_simple() {
        check_prepare_output(
            r#"BLABLABLA
foo.c : warning C4411: foo bar
"#,
            r#"foo.c : warning C4411: foo bar
"#, "BLABLABLA", true);
    }

    #[test]
    fn test_prepare_output_c4628_remove() {
        check_prepare_output(
            r#"BLABLABLA
foo.c(41) : warning C4411: foo bar
foo.c(42) : warning C4628: foo bar
foo.c(43) : warning C4433: foo bar
"#,
            r#"foo.c(41) : warning C4411: foo bar
foo.c(43) : warning C4433: foo bar
"#, "BLABLABLA", true);
    }

    #[test]
    fn test_prepare_output_c4628_keep() {
        check_prepare_output(
            r#"BLABLABLA
foo.c(41) : warning C4411: foo bar
foo.c(42) : warning C4628: foo bar
foo.c(43) : warning C4433: foo bar
"#,
            r#"foo.c(41) : warning C4411: foo bar
foo.c(42) : warning C4628: foo bar
foo.c(43) : warning C4433: foo bar
"#, "BLABLABLA", false);
    }
}
