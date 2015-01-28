use std::io::{Reader, Writer, IoError, IoErrorKind};

use super::super::utils::DEFAULT_BUF_SIZE;

#[derive(Show)]
enum Directive {
	// raw, file
	Line(Vec<u8>, String),
	// raw
	HdrStop(Vec<u8>),
	// raw
	Unknown(Vec<u8>)
}

pub fn filter_preprocessed(reader: &mut Reader, writer: &mut Writer, marker: &Option<String>, keep_headers: bool) -> Result<(), IoError> {
	let mut line_begin = true;
	// Entry file.
	let mut entry_file: Option<String> = None;
	let mut header_found: bool = false;
	loop {
		let c = try! (reader.read_u8());
		match c {
			b'\n' | b'\r' => {
				if keep_headers {
					try! (writer.write_u8(c));
				}
				line_begin = true;
			}
			b'\t' | b' ' => {
				if keep_headers {
					try! (writer.write_u8(c));
				}
			}
			b'#' if line_begin => {
				let directive = try! (read_directive(c, reader));
				match directive {
					Directive::Line(raw, raw_file) => {
						let file = raw_file.replace("\\", "/");
						entry_file = match entry_file {
							Some(path) => {
								if header_found && (path  == file) {
									try! (writer.write(b"#pragma hdrstop\n"));
									try! (writer.write(raw.as_slice()));
									break;
								}
								match *marker {
									Some(ref raw_path) => {
										let path = raw_path.replace("\\", "/");
										if file == path || Path::new(file.as_slice()).ends_with_path(&Path::new(path.as_slice())) {
											header_found = true;
										}
									}
									None => {}
								}
								Some(path)
							}
							None => Some(file)
						};
						if keep_headers {
							try! (writer.write(raw.as_slice()));
						}
					}
					Directive::HdrStop(raw) => {
						try! (writer.write(raw.as_slice()));
						break;
					}
					Directive::Unknown(raw) => {
						if keep_headers {
							try! (writer.write(raw.as_slice()));
						}
					}
				}
			}
			_ => {
				if keep_headers {
					try! (writer.write_u8(c));
				}
				line_begin = false;
			}
		}
	}
	// Copy end of stream.
	let mut buf: [u8; DEFAULT_BUF_SIZE] = [0; DEFAULT_BUF_SIZE];
	loop {
		match reader.read(&mut buf) {
			Ok(size) => {
				try! (writer.write(&buf.as_slice()[0..size]));
			}
			Err(ref e) if e.kind == IoErrorKind::EndOfFile => break,
			Err(e) => return Err(e)
		}
	}
	Ok(())
}

fn read_directive(first: u8, reader: &mut Reader) -> Result<Directive, IoError> {
	let mut raw: Vec<u8> = Vec::new();
	raw.push(first);
	let (next, token) = try! (read_token(None, reader, &mut raw));
	match token.as_slice() {
		b"line" => read_directive_line(next, reader, raw),
		b"pragma" => read_directive_pragma(next, reader, raw),
		_ => {
			try! (skip_line(next, reader, &mut raw));
			Ok(Directive::Unknown(raw))
		}
	}
}

fn read_token(first: Option<u8>, reader: &mut Reader, raw: &mut Vec<u8>) -> Result<(Option<u8>, Vec<u8>), IoError> {
	match try! (skip_spaces(first, reader, raw)) {
		Some(first_char) => {
			let mut token: Vec<u8> = Vec::new();
			let mut escape = false;
			let quote: bool;
			if first_char == b'"' {
				quote = true;
			} else {
				token.push(first_char);
				quote = false;
			}
			loop {
				let c = try! (reader.read_u8());
				raw.push(c);
				if quote {
					if escape {
						match c {
							b'n' => token.push(b'\n'),
							b'r' => token.push(b'\r'),
							b't' => token.push(b'\t'),
							v => token.push(v)
						}
						escape = false;
					} else if c == ('\\' as u8) {
						escape = true;
					} else if c == b'"' {
						let n = try! (reader.read_u8());
						raw.push(n);
						return Ok((Some(n), token));
					} else {
						token.push(c);
					}
				} else {
					match c {
						b'a' ... b'z' | b'A' ... b'Z' | b'0' ... b'9' => {
							token.push(c);
						}
						_ => {
							return Ok((Some(c), token));
						}
					}
				}
			}
		}
		None => {
			return Ok((None, Vec::new()));
		}
	}
}

fn read_directive_line(first: Option<u8>, reader: &mut Reader, mut raw: Vec<u8>) -> Result<Directive, IoError> {
	// Line number
	let (next1, _) = try! (read_token(first, reader, &mut raw));
	// File name
	let (next2, file) = try! (read_token(next1, reader, &mut raw));
	try! (skip_line(next2, reader, &mut raw));
	Ok(Directive::Line(raw, String::from_utf8_lossy(file.as_slice()).to_string()))
}

fn read_directive_pragma(first: Option<u8>, reader: &mut Reader, mut raw: Vec<u8>) -> Result<Directive, IoError> {
	let (next, token) = try! (read_token(first, reader, &mut raw));
	try! (skip_line(next, reader, &mut raw));
	match token.as_slice() {
		b"hdrstop" => Ok(Directive::HdrStop(raw)),
		_ => Ok(Directive::Unknown(raw))
	}
}

fn skip_spaces(first: Option<u8>, reader: &mut Reader, raw: &mut Vec<u8>) -> Result<Option<u8>, IoError> {
	match first {
		Some(c) => {
			match c {
				b'\n' | b'\r' => {return Ok(None);}
				b'\t' | b' ' => {}
				_ => {return Ok(first);}
			}
		}
		_ => {}
	}
	loop {
		let c = try! (reader.read_u8());
		try! (raw.write_u8(c));
		match c {
			b'\n' | b'\r' => {return Ok(None);}
			b'\t' | b' ' => {}
			_ => {return Ok(Some(c));}
		}
	}
}

fn skip_line(first: Option<u8>, reader: &mut Reader, raw: &mut Vec<u8>) -> Result<(), IoError> {
	match first {
		Some(c) => {
			match c {
				b'\n' | b'\r' => {return Ok(());}
				_ => {}
			}
		}
		_ => {}
	}
	loop {
		let c = try! (reader.read_u8());
		try! (raw.write_u8(c));
		match c {
			b'\n' | b'\r' => {return Ok(());}
			_ => {}
		}
	}
}

#[test]
fn test_filter_precompiled_keep() {
	let filtered = filter_preprocessed(br#"#line 1 "sample.cpp"
#line 1 "e:/work/octobuild/test_cl/sample header.h"
# pragma once
void hello();
#line 2 "sample.cpp"

int main(int argc, char **argv) {
	return 0;
}
"#, &Some("sample header.h".to_string()), true);
	assert_eq!(String::from_utf8_lossy(filtered.unwrap().as_slice()), r#"#line 1 "sample.cpp"
#line 1 "e:/work/octobuild/test_cl/sample header.h"
# pragma once
void hello();
#pragma hdrstop
#line 2 "sample.cpp"

int main(int argc, char **argv) {
	return 0;
}
"#);
}

#[test]
fn test_filter_precompiled_remove() {
	let filtered = filter_preprocessed(br#"#line 1 "sample.cpp"
#line 1 "e:/work/octobuild/test_cl/sample header.h"
# pragma once
void hello1();
void hello2();
#line 2 "sample.cpp"

int main(int argc, char **argv) {
	return 0;
}
"#, &Some("sample header.h".to_string()), false);
	assert_eq!(String::from_utf8_lossy(filtered.unwrap().as_slice()), r#"#pragma hdrstop
#line 2 "sample.cpp"

int main(int argc, char **argv) {
	return 0;
}
"#);
}

#[test]
fn test_filter_precompiled_hdrstop() {
	let filtered = filter_preprocessed(br#"#line 1 "sample.cpp"
 #line 1 "e:/work/octobuild/test_cl/sample header.h"
void hello();
# pragma  hdrstop
void data();
# pragma once
#line 2 "sample.cpp"

int main(int argc, char **argv) {
	return 0;
}
"#, &None, false);
	assert_eq!(String::from_utf8_lossy(filtered.unwrap().as_slice()), r#"# pragma  hdrstop
void data();
# pragma once
#line 2 "sample.cpp"

int main(int argc, char **argv) {
	return 0;
}
"#);
}

#[test]
fn test_filter_precompiled_xxx() {
	let filtered = filter_preprocessed(br#"#line 1 "sample.cpp"
#line 1 "e:\\work\\octobuild\\test_cl\\sample header.h"
# pragma once
void hello();
#line 2 "sample.cpp"

int main(int argc, char **argv) {
	return 0;
}
"#, &Some("e:\\work\\octobuild\\test_cl\\sample header.h".to_string()), true);
	assert_eq!(String::from_utf8_lossy(filtered.unwrap().as_slice()), r#"#line 1 "sample.cpp"
#line 1 "e:\\work\\octobuild\\test_cl\\sample header.h"
# pragma once
void hello();
#pragma hdrstop
#line 2 "sample.cpp"

int main(int argc, char **argv) {
	return 0;
}
"#);
}