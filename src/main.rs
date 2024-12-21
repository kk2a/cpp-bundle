use clap::{Arg, App};
use std::fs::{File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use regex::Regex;
use std::collections::HashSet;
use clipboard::{ClipboardContext, ClipboardProvider};

fn good_path(path: &str) -> PathBuf {
    Path::new(path).canonicalize().unwrap()
}

struct IncludeFile {
    file_path: PathBuf,
    include_path: PathBuf,
    user_list: Vec<String>,
    re: Regex
}

impl IncludeFile {
    fn new(file_path: &str, include_path: &str, user_list: Vec<String>) -> Self {
        Self {
            file_path: good_path(file_path),
            include_path: good_path(include_path),
            user_list,
            re: Regex::new(r"\s+").unwrap()
        }
    }

    fn get_include_path(&self, line: &str, cur_file_path: &Path) -> Option<PathBuf> {
        let replaced = self.re.replace_all(line, "");
        let trimmed = replaced.trim();

        if trimmed.starts_with("#include\"") {
            let path_str = trimmed.trim_start_matches("#include\"").trim_end_matches("\"");
            return Some(cur_file_path.parent().unwrap().join(path_str));
        }

        for user in &self.user_list {
            if !trimmed.starts_with(&format!("#include<{}/", user)) {
                continue;
            }
            let path_str = trimmed.trim_start_matches(&format!("#include<{}/", user)).trim_end_matches(">");
            return Some(self.include_path.join(user).join(path_str));
        }

        None
    }

    fn is_pragma_once(&self, line: &str) -> bool {
        line.trim().starts_with("#pragma once")
    }

    fn expand(&self, write: bool, clip: bool) {
        let mut file_path_set = HashSet::new();
        let mut lines = String::new();

        fn rec(
            cur_file_path: &Path,
            file_path_set: &mut HashSet<String>,
            lines: &mut String,
            include_obj: &IncludeFile,
        ) {
            if !file_path_set.insert(cur_file_path.to_str().unwrap().to_string()) {
                return;
            }
            // println!("expanding: {:?}", cur_file_path);

            if let Ok(file) = File::open(cur_file_path) {
                // println!("opened: {:?}", cur_file_path);
                let reader = BufReader::with_capacity(64 * 1024, file);
                for line in reader.lines() {
                    let line = line.unwrap();
                    if include_obj.is_pragma_once(&line) {
                        continue;
                    }
                    if let Some(included_file_path) = include_obj.get_include_path(&line, cur_file_path) {
                        rec(&included_file_path, file_path_set, lines, include_obj);
                        lines.push_str(&format!("// {}\n", line));
                    } else {
                        lines.push_str(&format!("{}\n", line));
                    }
                }
                lines.push_str("\n");
            }
        }

        rec(&self.file_path, &mut file_path_set, &mut lines, self);

        lines.push_str("// converted!!\n");
        if write {
            let mut file = File::create(&self.file_path).unwrap();
            file.write_all(lines.as_bytes()).unwrap();
        }
        if clip {
            let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
            ctx.set_contents(lines).unwrap();
        }
    }
}

fn main() {
    let matches = App::new("cpp-bundle")
        .version("1.0")
        .author("Your Name <your.email@example.com>")
        .about("Bundles C++ files")
        .arg(Arg::new("input")
            .help("Sets the input file to use")
            .required(true)
            .index(1))
        .arg(Arg::new("clip")
            .help("Copies the output to the clipboard")
            .short('c')
            .long("clip"))
        .arg(Arg::new("write")
            .help("Writes the output to the file")
            .short('w')
            .long("write"))
        .get_matches();

    let input_file = matches.value_of("input").unwrap();
    let clip = matches.is_present("clip");
    let write = matches.is_present("write");

    let include_path = "C:/Users/include/";
    let user_list = vec!["kk2".to_string()];
    let include_obj = IncludeFile::new(input_file, include_path, user_list);
    
    let start = std::time::Instant::now();
    include_obj.expand(write, clip);
    let end = std::time::Instant::now();
    println!("Elapsed: {:?}", end.duration_since(start));
}