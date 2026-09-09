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
            if !file_path_set.insert(cur_file_path.to_str().unwrap().to_string()) {
                return;
            }

            if let Ok(file) = File::open(cur_file_path) {
                let reader = BufReader::with_capacity(64 * 1024, file);
                for line in reader.lines() {
                    let line = line.unwrap();
                    let trimmed = include_obj.re.replace_all(&line, "").trim().to_string();
                    if trimmed.starts_with("#include") {
                        // ユーザーのヘッダーでない場合のみシステムヘッダーとして追加
                        if include_obj.get_include_path(&line, cur_file_path).is_none() {
                            system_headers.insert(line.clone());
                        } else {
                            rec_collect(
                                &include_obj.get_include_path(&line, cur_file_path).unwrap(),
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

    fn format_line(
        &self,
        line: &str,
        buffer_ends_with_newline: bool,
        preserve_newlines: bool,
    ) -> (String, bool) {
        // フォーマットが無効の場合は元の行をそのまま返す
        if !self.format_enabled {
            return (format!("{}\n", line), true);
        }

        // 改行保持モードでは空行も保持する
        if preserve_newlines {
            if buffer_ends_with_newline {
                return (format!("{}\n", line), true);
            } else {
                return (format!("\n{}\n", line), true);
            }
        }

        // 通常モードの処理
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return (String::new(), buffer_ends_with_newline);
        }

        // #で始まる行はそのまま保持
        if trimmed.starts_with('#') {
            if !buffer_ends_with_newline {
                return (format!("\n{}\n", trimmed), true);
            }
            return (format!("{}\n", trimmed), true);
        }

        // 通常の行は空白を単一スペースに変換
        let mut processed = self.re.replace_all(trimmed, " ").to_string();
        if let Some(comment_pos) = processed.find("//") {
            processed.truncate(comment_pos);
            processed = processed.trim_end().to_string();
        }
        (processed, false)
    }

    fn expand(&self, write: bool, clip: bool) {
        let (system_headers, mut file_path_set) = self.collect_all_headers();
        let mut lines = String::new();

        // Add all system headers at the beginning
        for header in system_headers {
            lines.push_str(&format!("{}\n", header));
        }
        lines.push_str("\n");

        file_path_set.clear();

        fn rec(
            cur_file_path: &Path,
            file_path_set: &mut HashSet<String>,
            lines: &mut String,
            include_obj: &IncludeFile,
            ends_with_newline: &mut bool,
            preserve_newlines: &mut bool,
        ) {
            if !file_path_set.insert(cur_file_path.to_str().unwrap().to_string()) {
                return;
            }

            if let Ok(file) = File::open(cur_file_path) {
                let reader = BufReader::with_capacity(64 * 1024, file);
                for line in reader.lines() {
                    let line = line.unwrap();

                    // 特殊コメントの検出
                    match line.trim() {
                        "// BEGIN_PRESERVE_NEWLINES" => {
                            *preserve_newlines = true;
                            continue;
                        }
                        "// END_PRESERVE_NEWLINES" => {
                            *preserve_newlines = false;
                            continue;
                        }
                        _ => {}
                    }

                    if include_obj.is_pragma_once(&line) {
                        continue;
                    }
                    if let Some(included_file_path) =
                        include_obj.get_include_path(&line, cur_file_path)
                    {
                        rec(
                            &included_file_path,
                            file_path_set,
                            lines,
                            include_obj,
                            ends_with_newline,
                            preserve_newlines,
                        );
                    } else if !line.trim().starts_with("#include") {
                        let (formatted, ends_nl) =
                            include_obj.format_line(&line, *ends_with_newline, *preserve_newlines);
                        if !formatted.is_empty() {
                            lines.push_str(&formatted);
                            if !ends_nl {
                                lines.push(' ');
                                *ends_with_newline = false;
                            } else {
                                *ends_with_newline = true;
                            }
                        }
                    }
                }
            }
        }

        let mut ends_with_newline = true;
        let mut preserve_newlines = false;
        rec(
            &self.file_path,
            &mut file_path_set,
            &mut lines,
            self,
            &mut ends_with_newline,
            &mut preserve_newlines,
        );

        if !ends_with_newline {
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

fn main() {
    let matches = App::new("cpp-bundle")
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
            Arg::new("no-format") // 追加: フォーマット無効化オプション
                .help("Disables code formatting")
                .long("no-format"),
        )
        .get_matches();

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
    let format_enabled = !matches.is_present("no-format"); // フォーマットフラグの設定

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
