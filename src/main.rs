use chrono::Local;
use clap::{App, Arg};
use clipboard::{ClipboardContext, ClipboardProvider};
use regex::Regex;
use std::collections::HashSet;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn good_path(path: &str) -> PathBuf {
    Path::new(path)
        .canonicalize()
        .unwrap_or_else(|error| panic!("failed to resolve path '{}': {}", path, error))
}

const LIBRARY_PATH_ENV: &str = "CP_LIBRARY_PATH";
const BEGIN_PRESERVE_NEWLINES: &str = "// BEGIN_PRESERVE_NEWLINES";
const END_PRESERVE_NEWLINES: &str = "// END_PRESERVE_NEWLINES";

fn is_identifier_start(byte: u8) -> bool {
    byte == b'_' || byte == b'$' || byte.is_ascii_alphabetic() || !byte.is_ascii()
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn consume_identifier(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && is_identifier_continue(bytes[pos]) {
        pos += 1;
    }
    pos
}

fn consume_literal_suffix(bytes: &[u8], pos: usize) -> usize {
    if pos < bytes.len() && is_identifier_start(bytes[pos]) {
        consume_identifier(bytes, pos)
    } else {
        pos
    }
}

fn literal_end(source: &str, start: usize) -> Option<usize> {
    const RAW_PREFIXES: &[&str] = &["u8R\"", "uR\"", "UR\"", "LR\"", "R\""];
    const QUOTED_PREFIXES: &[&str] = &[
        "u8\"", "u\"", "U\"", "L\"", "\"", "u8'", "u'", "U'", "L'", "'",
    ];

    let rest = &source[start..];
    let bytes = source.as_bytes();
    for prefix in RAW_PREFIXES {
        if !rest.starts_with(prefix) {
            continue;
        }
        let delimiter_start = start + prefix.len();
        let open_paren = source[delimiter_start..].find('(')? + delimiter_start;
        let delimiter = &source[delimiter_start..open_paren];
        if delimiter.len() > 16
            || delimiter
                .bytes()
                .any(|c| c.is_ascii_whitespace() || matches!(c, b'(' | b')' | b'\\'))
        {
            return None;
        }
        let closing = format!("){}\"", delimiter);
        let close_start = source[open_paren + 1..].find(&closing)? + open_paren + 1;
        return Some(consume_literal_suffix(bytes, close_start + closing.len()));
    }

    for prefix in QUOTED_PREFIXES {
        if !rest.starts_with(prefix) {
            continue;
        }
        let quote = prefix.as_bytes()[prefix.len() - 1];
        let mut pos = start + prefix.len();
        while pos < bytes.len() {
            if bytes[pos] == b'\\' {
                pos = (pos + 2).min(bytes.len());
            } else if bytes[pos] == quote {
                return Some(consume_literal_suffix(bytes, pos + 1));
            } else {
                pos += 1;
            }
        }
        return Some(bytes.len());
    }
    None
}

fn cpp_token_end(source: &str, start: usize) -> usize {
    const PUNCTUATORS: &[&str] = &[
        "%:%:", "<=>", ">>=", "<<=", "->*", "...", "##", "::", ".*", "->", "++", "--", "<<", ">>",
        "<=", ">=", "==", "!=", "&&", "||", "*=", "/=", "%=", "+=", "-=", "&=", "^=", "|=", "<:",
        ":>", "<%", "%>", "%:",
    ];

    if let Some(end) = literal_end(source, start) {
        return end;
    }

    let bytes = source.as_bytes();
    let first = bytes[start];
    if is_identifier_start(first) {
        return consume_identifier(bytes, start + 1);
    }
    if first.is_ascii_digit()
        || (first == b'.' && bytes.get(start + 1).is_some_and(u8::is_ascii_digit))
    {
        let mut pos = start + 1;
        while pos < bytes.len() {
            let byte = bytes[pos];
            let is_exponent_sign = matches!(byte, b'+' | b'-')
                && pos > start
                && matches!(bytes[pos - 1], b'e' | b'E' | b'p' | b'P');
            if is_identifier_continue(byte) || matches!(byte, b'.' | b'\'') || is_exponent_sign {
                pos += 1;
            } else {
                break;
            }
        }
        return pos;
    }
    for punctuator in PUNCTUATORS {
        if source[start..].starts_with(punctuator) {
            return start + punctuator.len();
        }
    }
    start + source[start..].chars().next().unwrap().len_utf8()
}

fn skip_whitespace_and_comments(source: &str, mut pos: usize) -> usize {
    let bytes = source.as_bytes();
    loop {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if source[pos..].starts_with("//") {
            pos = source[pos..]
                .find('\n')
                .map_or(bytes.len(), |offset| pos + offset + 1);
        } else if source[pos..].starts_with("/*") {
            pos = source[pos + 2..]
                .find("*/")
                .map_or(bytes.len(), |offset| pos + 2 + offset + 2);
        } else {
            return pos;
        }
    }
}

fn token_texts(source: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut pos = 0;
    while pos < source.len() {
        pos = skip_whitespace_and_comments(source, pos);
        if pos == source.len() {
            break;
        }
        let end = cpp_token_end(source, pos);
        result.push(&source[pos..end]);
        pos = end;
    }
    result
}

fn can_concatenate(left: &str, right: &str) -> bool {
    let joined = format!("{left}{right}");
    let tokens = token_texts(&joined);
    tokens.len() == 2 && tokens[0] == left && tokens[1] == right
}

fn defined_macro_names(source: &str) -> HashSet<&str> {
    source
        .lines()
        .filter_map(|line| {
            let directive = line.trim_start().strip_prefix('#')?.trim_start();
            let rest = directive.strip_prefix("define")?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            let definition = rest.trim_start();
            let end = definition
                .bytes()
                .position(|byte| !is_identifier_continue(byte))
                .unwrap_or(definition.len());
            (end > 0).then(|| &definition[..end])
        })
        .collect()
}

fn physical_line_end(source: &str, pos: usize) -> usize {
    source[pos..]
        .find('\n')
        .map_or(source.len(), |offset| pos + offset + 1)
}

fn is_marker_line(source: &str, pos: usize, marker: &str) -> bool {
    source[pos..physical_line_end(source, pos)].trim() == marker
}

fn directive_end(source: &str, mut pos: usize) -> usize {
    loop {
        let end = physical_line_end(source, pos);
        let line_without_newline = source[pos..end].trim_end_matches(['\r', '\n']);
        if end == source.len() || !line_without_newline.ends_with('\\') {
            return end;
        }
        pos = end;
    }
}

fn minify_cpp(source: &str) -> String {
    let macro_names = defined_macro_names(source);
    let mut output = String::new();
    let mut previous_token: Option<String> = None;
    let mut pos = 0;
    let mut at_line_start = true;
    let mut preserve_newlines = false;
    let mut pending_separator = false;
    let mut parenthesis_stack = Vec::new();
    let mut macro_argument_depth = 0;

    while pos < source.len() {
        if at_line_start && is_marker_line(source, pos, BEGIN_PRESERVE_NEWLINES) {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            previous_token = None;
            pending_separator = false;
            preserve_newlines = true;
            pos = physical_line_end(source, pos);
            continue;
        }
        if preserve_newlines {
            if at_line_start && is_marker_line(source, pos, END_PRESERVE_NEWLINES) {
                preserve_newlines = false;
                previous_token = None;
                pending_separator = false;
                pos = physical_line_end(source, pos);
                continue;
            }
            let end = physical_line_end(source, pos);
            output.push_str(&source[pos..end]);
            at_line_start = end > pos && source.as_bytes()[end - 1] == b'\n';
            pos = end;
            continue;
        }

        let byte = source.as_bytes()[pos];
        if byte.is_ascii_whitespace() {
            if byte == b'\n' {
                at_line_start = true;
            }
            pending_separator = true;
            pos += 1;
            continue;
        }
        if source[pos..].starts_with("//") {
            let end = physical_line_end(source, pos);
            at_line_start = end > pos && source.as_bytes()[end - 1] == b'\n';
            pending_separator = true;
            pos = end;
            continue;
        }
        if source[pos..].starts_with("/*") {
            let end = source[pos + 2..]
                .find("*/")
                .map_or(source.len(), |offset| pos + 2 + offset + 2);
            if source[pos..end].contains('\n') {
                at_line_start = true;
            }
            pending_separator = true;
            pos = end;
            continue;
        }
        if at_line_start && byte == b'#' {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            let end = directive_end(source, pos);
            output.push_str(&source[pos..end]);
            if end == source.len() && !output.ends_with('\n') {
                output.push('\n');
            }
            previous_token = None;
            pending_separator = false;
            at_line_start = true;
            pos = end;
            continue;
        }

        let end = cpp_token_end(source, pos);
        let token = &source[pos..end];
        if let Some(previous) = previous_token.as_deref() {
            if (pending_separator && macro_argument_depth > 0) || !can_concatenate(previous, token)
            {
                output.push(' ');
            }
        }
        output.push_str(token);
        if token == "(" {
            let is_macro_invocation = previous_token
                .as_deref()
                .is_some_and(|previous| macro_names.contains(previous));
            parenthesis_stack.push(is_macro_invocation);
            if is_macro_invocation {
                macro_argument_depth += 1;
            }
        } else if token == ")" && parenthesis_stack.pop().is_some_and(|is_macro| is_macro) {
            macro_argument_depth -= 1;
        }
        previous_token = Some(token.to_owned());
        pending_separator = false;
        at_line_start = false;
        pos = end;
    }

    output
}

fn include_guard_lines(lines: &[String]) -> Option<[usize; 3]> {
    if lines.len() < 3 {
        return None;
    }

    let guard = lines.first()?.strip_prefix("#ifndef ")?;
    if guard.is_empty()
        || !guard
            .chars()
            .all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return None;
    }
    if lines.get(1)? != &format!("#define {guard} 1") {
        return None;
    }

    let last = lines.len() - 1;
    if lines.get(last)? != &format!("#endif // {guard}") {
        return None;
    }
    Some([0, 1, last])
}

fn copy_to_clipboard(contents: &str) -> Result<(), String> {
    // VS Code を WSL で実行している場合は、Windows 側のクリップボードを使う。
    if env::var_os("WSL_INTEROP").is_some()
        || env::var_os("WSL_DISTRO_NAME").is_some()
        || Path::new("/mnt/c/windows/system32/clip.exe").exists()
    {
        // clip.exe は WSL からの日本語を安定して扱えるよう UTF-16LE で受け取る。
        // BOM はクリップボードの先頭に混入するため付けない。
        let utf16: Vec<u8> = contents
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let mut child = Command::new("clip.exe")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start clip.exe: {}", error))?;
        child
            .stdin
            .take()
            .ok_or_else(|| "failed to open clip.exe stdin".to_string())?
            .write_all(&utf16)
            .map_err(|error| format!("failed to write to clip.exe: {}", error))?;
        let status = child
            .wait()
            .map_err(|error| format!("failed to wait for clip.exe: {}", error))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("clip.exe exited with status {}", status));
    }

    let mut context: ClipboardContext = ClipboardProvider::new()
        .map_err(|error| format!("failed to access clipboard: {}", error))?;
    context
        .set_contents(contents.to_string())
        .map_err(|error| format!("failed to set clipboard contents: {}", error))
}

struct IncludeFile {
    file_path: PathBuf,
    include_path: PathBuf,
    re: Regex,
    author: String,
    format_enabled: bool, // 追加: フォーマットの有効/無効を制御
}

impl IncludeFile {
    fn new(file_path: &str, include_path: &str, author: String, format_enabled: bool) -> Self {
        Self {
            file_path: good_path(file_path),
            include_path: good_path(include_path),
            re: Regex::new(r"\s+").unwrap(),
            author,
            format_enabled,
        }
    }

    fn collect_all_headers(&self) -> (HashSet<String>, HashSet<String>) {
        let mut system_headers = HashSet::new();
        let mut file_path_set = HashSet::new();

        fn rec_collect(
            cur_file_path: &Path,
            file_path_set: &mut HashSet<String>,
            system_headers: &mut HashSet<String>,
            include_obj: &IncludeFile,
        ) {
            let canonical_path = cur_file_path
                .canonicalize()
                .unwrap_or_else(|_| cur_file_path.to_path_buf());
            if !file_path_set.insert(canonical_path.to_str().unwrap().to_string()) {
                return;
            }

            if let Ok(file) = File::open(&canonical_path) {
                let reader = BufReader::with_capacity(64 * 1024, file);
                for line in reader.lines() {
                    let line = line.unwrap();
                    let trimmed = include_obj.re.replace_all(&line, "").trim().to_string();
                    if trimmed.starts_with("#include") {
                        // ユーザーのヘッダーでない場合のみシステムヘッダーとして追加
                        if include_obj
                            .get_include_path(&line, &canonical_path)
                            .is_none()
                        {
                            system_headers.insert(line.clone());
                        } else {
                            rec_collect(
                                &include_obj
                                    .get_include_path(&line, &canonical_path)
                                    .unwrap(),
                                file_path_set,
                                system_headers,
                                include_obj,
                            );
                        }
                    }
                }
            }
        }

        rec_collect(
            &self.file_path,
            &mut file_path_set,
            &mut system_headers,
            self,
        );
        (system_headers, file_path_set)
    }

    fn get_include_path(&self, line: &str, cur_file_path: &Path) -> Option<PathBuf> {
        let replaced = self.re.replace_all(line, "");
        let trimmed = replaced.trim();

        if trimmed.starts_with("#include\"") {
            let path_str = trimmed
                .trim_start_matches("#include\"")
                .trim_end_matches("\"");
            return Some(cur_file_path.parent().unwrap().join(path_str));
        }

        if trimmed.starts_with("#include<") {
            let path_str = trimmed
                .trim_start_matches("#include<")
                .trim_end_matches(">");
            let include_path = self.include_path.join(path_str);
            if include_path.exists() {
                return Some(include_path);
            }
        }

        None
    }

    fn is_pragma_once(&self, line: &str) -> bool {
        line.trim().starts_with("#pragma once")
    }

    fn expand(&self, write: bool, clip: bool) {
        let (system_headers, mut file_path_set) = self.collect_all_headers();
        let mut lines = String::new();

        // Add all system headers at the beginning
        for header in system_headers {
            lines.push_str(&format!("{}\n", header));
        }
        lines.push('\n');

        file_path_set.clear();

        fn rec(
            cur_file_path: &Path,
            is_included_header: bool,
            file_path_set: &mut HashSet<String>,
            lines: &mut String,
            include_obj: &IncludeFile,
        ) {
            let canonical_path = cur_file_path
                .canonicalize()
                .unwrap_or_else(|_| cur_file_path.to_path_buf());
            if !file_path_set.insert(canonical_path.to_str().unwrap().to_string()) {
                return;
            }

            if let Ok(file) = File::open(&canonical_path) {
                let reader = BufReader::with_capacity(64 * 1024, file);
                let file_lines: Vec<String> = reader.lines().map(Result::unwrap).collect();
                let guard_lines = is_included_header
                    .then(|| include_guard_lines(&file_lines))
                    .flatten();
                for (line_index, line) in file_lines.into_iter().enumerate() {
                    if guard_lines.is_some_and(|indices| indices.contains(&line_index)) {
                        continue;
                    }

                    if include_obj.is_pragma_once(&line) {
                        continue;
                    }
                    if let Some(included_file_path) =
                        include_obj.get_include_path(&line, &canonical_path)
                    {
                        rec(&included_file_path, true, file_path_set, lines, include_obj);
                    } else if !line.trim().starts_with("#include") {
                        lines.push_str(&line);
                        lines.push('\n');
                    }
                }
            }
        }

        rec(&self.file_path, false, &mut file_path_set, &mut lines, self);

        if self.format_enabled {
            lines = minify_cpp(&lines);
        } else {
            lines = lines
                .lines()
                .filter(|line| {
                    !matches!(line.trim(), BEGIN_PRESERVE_NEWLINES | END_PRESERVE_NEWLINES)
                })
                .fold(String::new(), |mut output, line| {
                    output.push_str(line);
                    output.push('\n');
                    output
                });
        }
        if !lines.ends_with('\n') {
            lines.push('\n');
        }

        // メタデータ出力を最適化
        let now = Local::now();
        lines.push_str(&format!("// Author: {}\n", self.author));
        lines.push_str("// converted by https://github.com/kk2a/cpp-bundle\n");
        lines.push_str(&format!("// {}\n", now.format("%Y-%m-%d %H:%M:%S")));

        if write {
            let mut file = File::create(&self.file_path).unwrap();
            file.write_all(lines.as_bytes()).unwrap();
        }
        if clip {
            copy_to_clipboard(&lines).unwrap_or_else(|error| panic!("{}", error));
        }
    }
}

fn app() -> App<'static> {
    App::new("cpp-bundle")
        .about("Bundles C++ files")
        .arg(
            Arg::new("input")
                .help("Sets the input file to use")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("author")
                .help("Sets the author name")
                .required(true)
                .index(2),
        )
        .arg(
            Arg::new("clip")
                .help("Copies the output to the clipboard")
                .short('c')
                .long("clip"),
        )
        .arg(
            Arg::new("write")
                .help("Writes the output to the file")
                .short('w')
                .long("write"),
        )
        .arg(
            Arg::new("no-format")
                .help("Disables code formatting (enabled by default)")
                .long("no-format"),
        )
}

fn main() {
    let matches = app().get_matches();

    let input_file = matches.value_of("input").unwrap();
    let author = matches.value_of("author").unwrap();
    let include_path = env::var(LIBRARY_PATH_ENV).unwrap_or_else(|_| {
        eprintln!(
            "{} must be set to the C++ library directory",
            LIBRARY_PATH_ENV
        );
        std::process::exit(1);
    });
    let clip = matches.is_present("clip");
    let write = matches.is_present("write");
    let format_enabled = !matches.is_present("no-format");

    let include_obj = IncludeFile::new(
        input_file,
        &include_path,
        author.to_string(),
        format_enabled,
    );

    let start = std::time::Instant::now();
    include_obj.expand(write, clip);
    let end = std::time::Instant::now();
    println!("Elapsed: {:?}", end.duration_since(start));
}

#[cfg(test)]
mod tests {
    use super::{app, include_guard_lines, minify_cpp, token_texts};

    fn lines(source: &str) -> Vec<String> {
        source.lines().map(str::to_owned).collect()
    }

    #[test]
    fn formatting_is_enabled_by_default() {
        let matches = app()
            .try_get_matches_from(["cpp-bundle", "input.cpp", "author"])
            .unwrap();
        assert!(!matches.is_present("no-format"));
    }

    #[test]
    fn formatting_can_be_disabled_explicitly() {
        let matches = app()
            .try_get_matches_from(["cpp-bundle", "input.cpp", "author", "--no-format"])
            .unwrap();
        assert!(matches.is_present("no-format"));
    }

    #[test]
    fn detects_canonical_include_guard() {
        let source = lines(
            "#ifndef KK2_FPS_BASE_HPP\n#define KK2_FPS_BASE_HPP 1\nint value;\n#endif // KK2_FPS_BASE_HPP\n",
        );
        assert_eq!(include_guard_lines(&source), Some([0, 1, 3]));
    }

    #[test]
    fn rejects_mismatched_include_guard() {
        let source = lines(
            "#ifndef KK2_FPS_BASE_HPP\n#define KK2_OTHER_HPP 1\nint value;\n#endif // KK2_FPS_BASE_HPP\n",
        );
        assert_eq!(include_guard_lines(&source), None);
    }

    #[test]
    fn removes_comments_and_unnecessary_whitespace() {
        let source = "int  main() { // line comment\n int/**/value = 1 + 2; return value;\n}\n";
        assert_eq!(
            minify_cpp(source),
            "int main(){int value=1+2;return value;}"
        );
    }

    #[test]
    fn keeps_token_separating_space() {
        let source = "long long x; int y = x + +x; int z = x / *&y;";
        assert_eq!(minify_cpp(source), "long long x;int y=x+ +x;int z=x/ *&y;");
    }

    #[test]
    fn preserves_literals_verbatim() {
        let source = "const char *s = \"a  b // c\";\nauto r = R\"tag(x // y\n# z)tag\";\n";
        assert_eq!(
            minify_cpp(source),
            "const char*s=\"a  b // c\";auto r=R\"tag(x // y\n# z)tag\";"
        );
    }

    #[test]
    fn preserves_preprocessor_directives() {
        let source = "#define F(x) \\\n  ((x) + 1) // macro comment\nint x = F(1);\n";
        assert_eq!(
            minify_cpp(source),
            "#define F(x) \\\n  ((x) + 1) // macro comment\nint x=F(1);"
        );
    }

    #[test]
    fn preserves_explicit_region_and_removes_markers() {
        let source = "int x;\n// BEGIN_PRESERVE_NEWLINES\nint main() {\n  return 0;\n}\n// END_PRESERVE_NEWLINES\n";
        assert_eq!(minify_cpp(source), "int x;\nint main() {\n  return 0;\n}\n");
    }

    #[test]
    fn recognizes_user_defined_literal_as_one_token() {
        assert_eq!(token_texts("\"x\"_suffix"), vec!["\"x\"_suffix"]);
        assert_eq!(
            minify_cpp("auto x = \"x\" _suffix;"),
            "auto x=\"x\" _suffix;"
        );
    }

    #[test]
    fn preserves_whitespace_that_a_macro_could_stringize() {
        let source = "#define STR(x) #x\nSTR(a + b); function(a + b); if (a + b) return;";
        assert_eq!(
            minify_cpp(source),
            "#define STR(x) #x\nSTR(a + b);function(a+b);if(a+b)return;"
        );
    }

    #[test]
    fn removes_whitespace_inside_function_parentheses() {
        let source = "Yes(bool b = 1); kout << kk2::sum_of_geometric_monomial(r, n, d) << kendl; bool chmin(T &a, const S &b);";
        assert_eq!(
            minify_cpp(source),
            "Yes(bool b=1);kout<<kk2::sum_of_geometric_monomial(r,n,d)<<kendl;bool chmin(T&a,const S&b);"
        );
    }
}
