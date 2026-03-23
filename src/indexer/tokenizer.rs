use regex::Regex;
use std::collections::HashSet;

lazy_static::lazy_static! {
    static ref CAMEL_CASE: Regex = Regex::new(r"([a-z])([A-Z])").unwrap();
    static ref SNAKE_CASE: Regex = Regex::new(r"_+").unwrap();
    static ref NON_ALPHANUM: Regex = Regex::new(r"[^a-zA-Z0-9]+").unwrap();
    static ref KEYWORDS: HashSet<&'static str> = {
        let set: HashSet<&'static str> = [
            "fn", "function", "func", "def", "class", "struct", "enum", "interface",
            "let", "const", "var", "val", "mut", "static", "final", "public", "private",
            "return", "yield", "await", "async", "sync", "if", "else", "elif", "for",
            "while", "loop", "match", "switch", "case", "break", "continue", "goto",
            "import", "export", "use", "require", "include", "from", "package", "mod",
            "impl", "trait", "extends", "implements", "where", "type", "alias", "typedef",
            "new", "delete", "malloc", "free", "this", "self", "super", "parent",
            "true", "false", "null", "nil", "none", "undefined", "void",
            "int", "float", "double", "string", "bool", "boolean", "char", "byte",
            "str", "vec", "map", "set", "list", "array", "dict", "hash",
            "error", "result", "option", "some", "ok", "err",
        ].iter().cloned().collect();
        set
    };
}

pub fn tokenize_code(content: &str) -> Vec<String> {
    let mut tokens = Vec::new();

    let content = remove_comments(content);

    for line in content.lines() {
        let line_tokens = tokenize_line(line);
        tokens.extend(line_tokens);
    }

    tokens.sort();
    tokens.dedup();
    tokens
}

pub fn tokenize_query(query: &str) -> String {
    let tokens = tokenize_code(query);
    tokens.join(" ")
}

fn tokenize_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();

    let line = line.trim();

    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
        return tokens;
    }

    let words = NON_ALPHANUM.split(line);

    for word in words {
        if word.is_empty() {
            continue;
        }

        let split_camel = CAMEL_CASE.replace_all(word, "$1 $2");
        let split_words: Vec<&str> = split_camel.split_whitespace().collect();

        for split_word in split_words {
            let lower = split_word.to_lowercase();

            if lower.len() < 2 {
                continue;
            }

            if KEYWORDS.contains(lower.as_str()) {
                tokens.push(lower);
                continue;
            }

            if lower.contains('_') {
                for part in SNAKE_CASE.split(&lower) {
                    if part.len() >= 2 {
                        tokens.push(part.to_string());
                    }
                }
            } else {
                tokens.push(lower);
            }
        }
    }

    tokens
}

fn remove_comments(content: &str) -> String {
    let mut result = String::new();
    let mut in_block_comment = false;
    let mut in_string = false;
    let mut string_char = '"';
    let mut prev_char = '\0';

    for line in content.lines() {
        let mut cleaned_line = String::new();
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let c = chars[i];
            let next_c = chars.get(i + 1).copied().unwrap_or('\0');

            if in_block_comment {
                if c == '*' && next_c == '/' {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_string {
                cleaned_line.push(c);
                if c == string_char && prev_char != '\\' {
                    in_string = false;
                }
                prev_char = c;
                i += 1;
                continue;
            }

            if c == '"' || c == '\'' || c == '`' {
                in_string = true;
                string_char = c;
                cleaned_line.push(c);
                prev_char = c;
                i += 1;
                continue;
            }

            if c == '/' && next_c == '/' {
                break;
            }

            if c == '/' && next_c == '*' {
                in_block_comment = true;
                i += 2;
                continue;
            }

            if c == '#' {
                break;
            }

            cleaned_line.push(c);
            prev_char = c;
            i += 1;
        }

        result.push_str(&cleaned_line);
        result.push('\n');
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_camel_case() {
        let tokens = tokenize_line("myFunctionName");
        assert!(tokens.contains(&"my".to_string()));
        assert!(tokens.contains(&"function".to_string()));
        assert!(tokens.contains(&"name".to_string()));
    }

    #[test]
    fn test_tokenize_snake_case() {
        let tokens = tokenize_line("my_variable_name");
        assert!(tokens.contains(&"my".to_string()));
        assert!(tokens.contains(&"variable".to_string()));
        assert!(tokens.contains(&"name".to_string()));
    }

    #[test]
    fn test_tokenize_mixed() {
        let tokens = tokenize_line("my_functionName mixedCase");
        assert!(tokens.contains(&"my".to_string()));
        assert!(tokens.contains(&"function".to_string()));
        assert!(tokens.contains(&"name".to_string()));
        assert!(tokens.contains(&"mixed".to_string()));
        assert!(tokens.contains(&"case".to_string()));
    }

    #[test]
    fn test_remove_line_comments() {
        let code = "let x = 5; // this is a comment\nlet y = 10;";
        let cleaned = remove_comments(code);
        assert!(!cleaned.contains("comment"));
        assert!(cleaned.contains("let x = 5;"));
        assert!(cleaned.contains("let y = 10;"));
    }

    #[test]
    fn test_remove_block_comments() {
        let code = "let x = /* comment */ 5;";
        let cleaned = remove_comments(code);
        assert!(!cleaned.contains("comment"));
        assert!(cleaned.contains("let x =  5;"));
    }
}
