use super::ReplMode;

const RESET: &[u8] = b"\x1b[0m";
const COMMENT: &[u8] = b"\x1b[2m";
const KEYWORD: &[u8] = b"\x1b[35m";
const TYPE: &[u8] = b"\x1b[34m";
const STRING: &[u8] = b"\x1b[32m";
const NUMBER: &[u8] = b"\x1b[33m";
const SPECIAL: &[u8] = b"\x1b[36m";

pub(super) fn highlight(mode: ReplMode, text: &[u8]) -> Vec<u8> {
    match mode {
        ReplMode::Python => highlight_python(text),
        ReplMode::Shell => highlight_shell(text),
        ReplMode::Sql => highlight_sql(text),
        ReplMode::C => highlight_c(text),
        ReplMode::Js => text.to_vec(),
    }
}

fn styled(out: &mut Vec<u8>, style: &[u8], text: &[u8]) {
    if text.is_empty() {
        return;
    }
    out.extend_from_slice(style);
    out.extend_from_slice(text);
    out.extend_from_slice(RESET);
}

fn is_identifier_start(ch: u8) -> bool {
    ch.is_ascii_alphabetic() || ch == b'_' || ch >= 0x80
}

fn is_identifier_continue(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'_' || ch >= 0x80
}

fn scan_identifier(text: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < text.len() && is_identifier_continue(text[i]) {
        i += 1;
    }
    i
}

fn scan_number(text: &[u8], start: usize) -> usize {
    let mut i = start;
    if text.get(i) == Some(&b'.') {
        i += 1;
    }

    if i + 1 < text.len() && text[i] == b'0' {
        let radix = text[i + 1].to_ascii_lowercase();
        if matches!(radix, b'b' | b'o' | b'x') {
            i += 2;
            while i < text.len() && (text[i].is_ascii_hexdigit() || text[i] == b'_') {
                i += 1;
            }
            if radix == b'x' && text.get(i) == Some(&b'.') {
                i += 1;
                while i < text.len() && (text[i].is_ascii_hexdigit() || text[i] == b'_') {
                    i += 1;
                }
            }
            if radix == b'x' && matches!(text.get(i), Some(b'p') | Some(b'P')) {
                i += 1;
                if matches!(text.get(i), Some(b'+') | Some(b'-')) {
                    i += 1;
                }
                while i < text.len() && (text[i].is_ascii_digit() || text[i] == b'_') {
                    i += 1;
                }
            }
            while i < text.len() && text[i].is_ascii_alphabetic() {
                i += 1;
            }
            return i;
        }
    }

    while i < text.len() && (text[i].is_ascii_digit() || text[i] == b'_') {
        i += 1;
    }
    if i < text.len() && text[i] == b'.' && text.get(i + 1) != Some(&b'.') {
        i += 1;
        while i < text.len() && (text[i].is_ascii_digit() || text[i] == b'_') {
            i += 1;
        }
    }
    if i < text.len() && matches!(text[i], b'e' | b'E' | b'p' | b'P') {
        i += 1;
        if i < text.len() && matches!(text[i], b'+' | b'-') {
            i += 1;
        }
        while i < text.len() && (text[i].is_ascii_digit() || text[i] == b'_') {
            i += 1;
        }
    }
    while i < text.len() && text[i].is_ascii_alphabetic() {
        i += 1;
    }
    i
}

fn scan_quoted(
    text: &[u8],
    quote_start: usize,
    allow_backslash_escape: bool,
    allow_doubled_quote: bool,
    allow_triple_quote: bool,
) -> usize {
    let quote = text[quote_start];
    let triple = allow_triple_quote
        && text.get(quote_start + 1) == Some(&quote)
        && text.get(quote_start + 2) == Some(&quote);
    let delimiter_len = if triple { 3 } else { 1 };
    let mut i = quote_start + delimiter_len;

    while i < text.len() {
        if allow_backslash_escape && text[i] == b'\\' {
            i = (i + 2).min(text.len());
            continue;
        }
        if allow_doubled_quote && text[i] == quote && text.get(i + 1) == Some(&quote) {
            i += 2;
            continue;
        }
        if triple {
            if text[i] == quote
                && text.get(i + 1) == Some(&quote)
                && text.get(i + 2) == Some(&quote)
            {
                return i + 3;
            }
        } else if text[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    text.len()
}

fn next_non_whitespace(text: &[u8], mut i: usize) -> Option<u8> {
    while i < text.len() && text[i].is_ascii_whitespace() {
        i += 1;
    }
    text.get(i).copied()
}

fn line_end(text: &[u8], start: usize) -> usize {
    text[start..]
        .iter()
        .position(|ch| *ch == b'\n')
        .map_or(text.len(), |offset| start + offset)
}

fn matching_operator(text: &[u8], start: usize, operators: &[&[u8]]) -> usize {
    operators
        .iter()
        .find(|operator| text[start..].starts_with(operator))
        .map_or(0, |operator| operator.len())
}

// Python --------------------------------------------------------------------

fn python_string_quote(text: &[u8], start: usize) -> Option<usize> {
    if matches!(text[start], b'\'' | b'"') {
        return Some(start);
    }

    for prefix_len in [2, 1] {
        let quote_start = start + prefix_len;
        let Some(quote @ (b'\'' | b'"')) = text.get(quote_start).copied() else {
            continue;
        };
        let prefix = &text[start..quote_start];
        let valid = match prefix_len {
            1 => matches!(prefix[0].to_ascii_lowercase(), b'b' | b'f' | b'r' | b'u'),
            2 => {
                let a = prefix[0].to_ascii_lowercase();
                let b = prefix[1].to_ascii_lowercase();
                matches!(
                    (a, b),
                    (b'b', b'r') | (b'r', b'b') | (b'f', b'r') | (b'r', b'f')
                )
            }
            _ => false,
        };
        if valid && matches!(quote, b'\'' | b'"') {
            return Some(quote_start);
        }
    }
    None
}

fn is_python_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"and"
            | b"as"
            | b"assert"
            | b"async"
            | b"await"
            | b"break"
            | b"case"
            | b"class"
            | b"continue"
            | b"def"
            | b"del"
            | b"elif"
            | b"else"
            | b"except"
            | b"finally"
            | b"for"
            | b"from"
            | b"global"
            | b"if"
            | b"import"
            | b"in"
            | b"is"
            | b"lambda"
            | b"match"
            | b"nonlocal"
            | b"not"
            | b"or"
            | b"pass"
            | b"raise"
            | b"return"
            | b"try"
            | b"type"
            | b"while"
            | b"with"
            | b"yield"
    )
}

fn is_python_builtin(word: &[u8]) -> bool {
    matches!(
        word,
        b"__import__"
            | b"abs"
            | b"aiter"
            | b"all"
            | b"anext"
            | b"any"
            | b"ascii"
            | b"bin"
            | b"bool"
            | b"breakpoint"
            | b"bytearray"
            | b"bytes"
            | b"callable"
            | b"chr"
            | b"classmethod"
            | b"compile"
            | b"complex"
            | b"delattr"
            | b"dict"
            | b"dir"
            | b"divmod"
            | b"enumerate"
            | b"eval"
            | b"exec"
            | b"filter"
            | b"float"
            | b"format"
            | b"frozenset"
            | b"getattr"
            | b"globals"
            | b"hasattr"
            | b"hash"
            | b"help"
            | b"hex"
            | b"id"
            | b"input"
            | b"int"
            | b"isinstance"
            | b"issubclass"
            | b"iter"
            | b"len"
            | b"list"
            | b"locals"
            | b"map"
            | b"max"
            | b"memoryview"
            | b"min"
            | b"next"
            | b"object"
            | b"oct"
            | b"open"
            | b"ord"
            | b"pow"
            | b"print"
            | b"property"
            | b"range"
            | b"repr"
            | b"reversed"
            | b"round"
            | b"set"
            | b"setattr"
            | b"slice"
            | b"sorted"
            | b"staticmethod"
            | b"str"
            | b"sum"
            | b"super"
            | b"tuple"
            | b"vars"
            | b"zip"
    )
}

fn highlight_python(text: &[u8]) -> Vec<u8> {
    const OPERATORS: &[&[u8]] = &[
        b"**=", b"//=", b">>=", b"<<=", b":=", b"==", b"!=", b"<=", b">=", b"->", b"**", b"//",
        b"<<", b">>", b"+=", b"-=", b"*=", b"/=", b"%=", b"&=", b"|=", b"^=", b"+", b"-", b"*",
        b"/", b"%", b"@", b"&", b"|", b"^", b"~", b"<", b">", b"=",
    ];

    let mut out = Vec::with_capacity(text.len() + 64);
    let mut i = 0;
    let mut definition_name = false;
    while i < text.len() {
        let ch = text[i];
        if ch == b'#' {
            let end = line_end(text, i);
            styled(&mut out, COMMENT, &text[i..end]);
            i = end;
            continue;
        }
        if let Some(quote_start) = python_string_quote(text, i) {
            let end = scan_quoted(text, quote_start, true, false, true);
            styled(&mut out, STRING, &text[i..end]);
            i = end;
            definition_name = false;
            continue;
        }
        if ch == b'@' && text.get(i + 1).copied().is_some_and(is_identifier_start) {
            let start = i;
            i += 1;
            while i < text.len() && (is_identifier_continue(text[i]) || text[i] == b'.') {
                i += 1;
            }
            styled(&mut out, SPECIAL, &text[start..i]);
            continue;
        }
        if ch.is_ascii_digit() || (ch == b'.' && text.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let end = scan_number(text, i);
            styled(&mut out, NUMBER, &text[i..end]);
            i = end;
            definition_name = false;
            continue;
        }
        if is_identifier_start(ch) {
            let end = scan_identifier(text, i);
            let word = &text[i..end];
            if definition_name {
                styled(&mut out, TYPE, word);
                definition_name = false;
            } else if is_python_keyword(word) {
                styled(&mut out, KEYWORD, word);
                definition_name = matches!(word, b"class" | b"def");
            } else if matches!(
                word,
                b"False" | b"None" | b"True" | b"Ellipsis" | b"NotImplemented"
            ) {
                styled(&mut out, NUMBER, word);
            } else if is_python_builtin(word) || next_non_whitespace(text, end) == Some(b'(') {
                styled(&mut out, TYPE, word);
            } else {
                out.extend_from_slice(word);
            }
            i = end;
            continue;
        }
        let operator_len = matching_operator(text, i, OPERATORS);
        if operator_len > 0 {
            styled(&mut out, SPECIAL, &text[i..i + operator_len]);
            i += operator_len;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

// Shell ---------------------------------------------------------------------

fn is_shell_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"case"
            | b"coproc"
            | b"do"
            | b"done"
            | b"elif"
            | b"else"
            | b"esac"
            | b"fi"
            | b"for"
            | b"function"
            | b"if"
            | b"in"
            | b"select"
            | b"then"
            | b"time"
            | b"until"
            | b"while"
    )
}

fn scan_shell_expansion(text: &[u8], start: usize) -> usize {
    if text.get(start + 1) == Some(&b'{') {
        let mut depth = 1usize;
        let mut i = start + 2;
        while i < text.len() {
            match text[i] {
                b'\\' => i = (i + 2).min(text.len()),
                b'{' => {
                    depth += 1;
                    i += 1;
                }
                b'}' => {
                    depth -= 1;
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => i += 1,
            }
        }
        return i;
    }
    if text.get(start + 1) == Some(&b'(') {
        let mut depth = 1usize;
        let mut i = start + 2;
        while i < text.len() {
            match text[i] {
                b'\\' => i = (i + 2).min(text.len()),
                b'\'' | b'"' => i = scan_quoted(text, i, true, false, false),
                b'(' => {
                    depth += 1;
                    i += 1;
                }
                b')' => {
                    depth -= 1;
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => i += 1,
            }
        }
        return i;
    }
    let Some(next) = text.get(start + 1).copied() else {
        return start + 1;
    };
    if matches!(
        next,
        b'0'..=b'9' | b'@' | b'*' | b'#' | b'?' | b'-' | b'$' | b'!'
    ) {
        return start + 2;
    }
    if is_identifier_start(next) {
        return scan_identifier(text, start + 1);
    }
    start + 1
}

fn highlight_shell_double_quote(text: &[u8], start: usize, out: &mut Vec<u8>) -> usize {
    let mut segment = start;
    let mut i = start + 1;
    while i < text.len() {
        if text[i] == b'\\' {
            i = (i + 2).min(text.len());
            continue;
        }
        if text[i] == b'$' {
            styled(out, STRING, &text[segment..i]);
            let end = scan_shell_expansion(text, i);
            styled(out, SPECIAL, &text[i..end]);
            i = end;
            segment = end;
            continue;
        }
        if text[i] == b'"' {
            i += 1;
            styled(out, STRING, &text[segment..i]);
            return i;
        }
        i += 1;
    }
    styled(out, STRING, &text[segment..]);
    text.len()
}

fn is_shell_word_delimiter(ch: u8) -> bool {
    ch.is_ascii_whitespace()
        || matches!(
            ch,
            b'\'' | b'"' | b'`' | b'$' | b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>'
        )
}

fn is_shell_assignment(word: &[u8]) -> Option<usize> {
    let equals = word.iter().position(|ch| *ch == b'=')?;
    if equals == 0 || !is_identifier_start(word[0]) {
        return None;
    }
    word[1..equals]
        .iter()
        .all(|ch| is_identifier_continue(*ch))
        .then_some(equals)
}

fn highlight_shell(text: &[u8]) -> Vec<u8> {
    const OPERATORS: &[&[u8]] = &[
        b"<<<", b"<<-", b"&>>", b";;&", b"&&", b"||", b";;", b";&", b"|&", b">>", b"<<", b"<>",
        b">|", b">&", b"<&", b"&>", b"[[", b"]]", b"<", b">", b"|", b"&", b";", b"(", b")", b"!",
        b"{", b"}",
    ];

    let mut out = Vec::with_capacity(text.len() + 64);
    let mut i = 0;
    let mut command_position = true;
    let mut word_start = true;
    let mut assignment_word = false;
    while i < text.len() {
        let ch = text[i];
        if ch.is_ascii_whitespace() {
            out.push(ch);
            if ch == b'\n' {
                command_position = true;
            }
            assignment_word = false;
            word_start = true;
            i += 1;
            continue;
        }
        if ch == b'#' && word_start {
            let end = line_end(text, i);
            styled(&mut out, COMMENT, &text[i..end]);
            i = end;
            continue;
        }
        if ch == b'\'' {
            let end = scan_quoted(text, i, false, false, false);
            styled(&mut out, STRING, &text[i..end]);
            i = end;
            if !assignment_word {
                command_position = false;
            }
            word_start = false;
            continue;
        }
        if ch == b'"' {
            i = highlight_shell_double_quote(text, i, &mut out);
            if !assignment_word {
                command_position = false;
            }
            word_start = false;
            continue;
        }
        if ch == b'`' {
            let end = scan_quoted(text, i, true, false, false);
            styled(&mut out, SPECIAL, &text[i..end]);
            i = end;
            if !assignment_word {
                command_position = false;
            }
            word_start = false;
            continue;
        }
        if ch == b'$' {
            let end = scan_shell_expansion(text, i);
            styled(&mut out, SPECIAL, &text[i..end]);
            i = end;
            if !assignment_word {
                command_position = false;
            }
            word_start = false;
            continue;
        }
        if ch.is_ascii_digit() {
            let mut fd_end = i;
            while fd_end < text.len() && text[fd_end].is_ascii_digit() {
                fd_end += 1;
            }
            if matches!(text.get(fd_end), Some(b'<') | Some(b'>')) {
                let operator_len = matching_operator(text, fd_end, OPERATORS);
                let mut end = fd_end + operator_len;
                if matches!(&text[fd_end..end], b">&" | b"<&") {
                    while end < text.len() && (text[end].is_ascii_digit() || text[end] == b'-') {
                        end += 1;
                    }
                }
                styled(&mut out, NUMBER, &text[i..end]);
                i = end;
                word_start = true;
                continue;
            }
        }
        let operator_len = matching_operator(text, i, OPERATORS);
        if operator_len > 0 {
            let operator = &text[i..i + operator_len];
            styled(
                &mut out,
                if matches!(operator.first(), Some(b'<') | Some(b'>')) {
                    NUMBER
                } else {
                    KEYWORD
                },
                operator,
            );
            if !matches!(operator.first(), Some(b'<') | Some(b'>')) {
                command_position = true;
                assignment_word = false;
            }
            word_start = true;
            i += operator_len;
            continue;
        }

        let start = i;
        while i < text.len() && !is_shell_word_delimiter(text[i]) {
            i += 1;
        }
        let word = &text[start..i];
        if let Some(equals) = is_shell_assignment(word) {
            styled(&mut out, SPECIAL, &word[..equals]);
            out.extend_from_slice(&word[equals..]);
            assignment_word = true;
        } else if is_shell_keyword(word) {
            styled(&mut out, KEYWORD, word);
            command_position = matches!(
                word,
                b"do" | b"elif" | b"else" | b"if" | b"then" | b"while" | b"until"
            );
            assignment_word = false;
        } else if word.starts_with(b"-") {
            styled(&mut out, NUMBER, word);
            command_position = false;
            assignment_word = false;
        } else if command_position {
            styled(&mut out, TYPE, word);
            command_position = false;
            assignment_word = false;
        } else {
            out.extend_from_slice(word);
            assignment_word = false;
        }
        word_start = false;
    }
    out
}

// SQL -----------------------------------------------------------------------

const SQL_KEYWORDS: &[&[u8]] = &[
    b"ABORT",
    b"ACTION",
    b"ADD",
    b"AFTER",
    b"ALL",
    b"ALTER",
    b"ANALYZE",
    b"AND",
    b"AS",
    b"ASC",
    b"ATTACH",
    b"AUTOINCREMENT",
    b"BEFORE",
    b"BEGIN",
    b"BETWEEN",
    b"BY",
    b"CASCADE",
    b"CASE",
    b"CAST",
    b"CHECK",
    b"COLLATE",
    b"COLUMN",
    b"COMMIT",
    b"CONFLICT",
    b"CONSTRAINT",
    b"CREATE",
    b"CROSS",
    b"CURRENT",
    b"CURRENT_DATE",
    b"CURRENT_TIME",
    b"CURRENT_TIMESTAMP",
    b"DATABASE",
    b"DEFAULT",
    b"DEFERRABLE",
    b"DEFERRED",
    b"DELETE",
    b"DESC",
    b"DETACH",
    b"DISTINCT",
    b"DO",
    b"DROP",
    b"EACH",
    b"ELSE",
    b"END",
    b"ESCAPE",
    b"EXCEPT",
    b"EXCLUDE",
    b"EXCLUSIVE",
    b"EXISTS",
    b"EXPLAIN",
    b"FAIL",
    b"FILTER",
    b"FIRST",
    b"FOLLOWING",
    b"FOR",
    b"FOREIGN",
    b"FROM",
    b"FULL",
    b"GENERATED",
    b"GLOB",
    b"GROUP",
    b"GROUPS",
    b"HAVING",
    b"IF",
    b"IGNORE",
    b"IMMEDIATE",
    b"IN",
    b"INDEX",
    b"INDEXED",
    b"INITIALLY",
    b"INNER",
    b"INSERT",
    b"INSTEAD",
    b"INTERSECT",
    b"INTO",
    b"IS",
    b"ISNULL",
    b"JOIN",
    b"KEY",
    b"LAST",
    b"LEFT",
    b"LIKE",
    b"LIMIT",
    b"MATCH",
    b"MATERIALIZED",
    b"NATURAL",
    b"NO",
    b"NOT",
    b"NOTHING",
    b"NOTNULL",
    b"NULL",
    b"NULLS",
    b"OF",
    b"OFFSET",
    b"ON",
    b"OR",
    b"ORDER",
    b"OTHERS",
    b"OUTER",
    b"OVER",
    b"PARTITION",
    b"PLAN",
    b"PRAGMA",
    b"PRECEDING",
    b"PRIMARY",
    b"QUERY",
    b"RAISE",
    b"RANGE",
    b"RECURSIVE",
    b"REFERENCES",
    b"REGEXP",
    b"REINDEX",
    b"RELEASE",
    b"RENAME",
    b"REPLACE",
    b"RESTRICT",
    b"RETURNING",
    b"RIGHT",
    b"ROLLBACK",
    b"ROW",
    b"ROWS",
    b"SAVEPOINT",
    b"SELECT",
    b"SET",
    b"TABLE",
    b"TEMP",
    b"TEMPORARY",
    b"THEN",
    b"TIES",
    b"TO",
    b"TRANSACTION",
    b"TRIGGER",
    b"UNBOUNDED",
    b"UNION",
    b"UNIQUE",
    b"UPDATE",
    b"USING",
    b"VACUUM",
    b"VALUES",
    b"VIEW",
    b"VIRTUAL",
    b"WHEN",
    b"WHERE",
    b"WINDOW",
    b"WITH",
    b"WITHOUT",
];

fn sql_word_is(word: &[u8], expected: &[u8]) -> bool {
    word.eq_ignore_ascii_case(expected)
}

fn is_sql_keyword(word: &[u8]) -> bool {
    const MAX_KEYWORD_LENGTH: usize = b"CURRENT_TIMESTAMP".len();
    if word.len() > MAX_KEYWORD_LENGTH {
        return false;
    }

    let mut uppercase = [0u8; MAX_KEYWORD_LENGTH];
    for (target, source) in uppercase.iter_mut().zip(word) {
        *target = source.to_ascii_uppercase();
    }
    let uppercase = &uppercase[..word.len()];
    SQL_KEYWORDS
        .binary_search_by(|keyword| (*keyword).cmp(uppercase))
        .is_ok()
}

fn scan_sql_parameter(text: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    if text[start] == b'?' {
        while i < text.len() && text[i].is_ascii_digit() {
            i += 1;
        }
        return i;
    }
    while i < text.len()
        && (is_identifier_continue(text[i]) || matches!(text[i], b':' | b'(' | b')'))
    {
        i += 1;
    }
    i
}

fn highlight_sql(text: &[u8]) -> Vec<u8> {
    const OPERATORS: &[&[u8]] = &[
        b"->>", b"->", b"||", b"<<", b">>", b"<=", b">=", b"==", b"!=", b"<>", b"+", b"-", b"*",
        b"/", b"%", b"&", b"|", b"~", b"<", b">", b"=",
    ];

    let mut out = Vec::with_capacity(text.len() + 64);
    let mut i = 0;
    let mut line_start = true;
    while i < text.len() {
        let ch = text[i];
        if ch.is_ascii_whitespace() {
            out.push(ch);
            line_start = ch == b'\n' || line_start;
            i += 1;
            continue;
        }
        if text[i..].starts_with(b"--") {
            let end = line_end(text, i);
            styled(&mut out, COMMENT, &text[i..end]);
            i = end;
            continue;
        }
        if text[i..].starts_with(b"/*") {
            let mut end = i + 2;
            while end + 1 < text.len() && !text[end..].starts_with(b"*/") {
                end += 1;
            }
            end = (end + 2).min(text.len());
            styled(&mut out, COMMENT, &text[i..end]);
            line_start = line_start || text[i..end].contains(&b'\n');
            i = end;
            continue;
        }
        if line_start && ch == b'.' && text.get(i + 1).copied().is_some_and(is_identifier_start) {
            let mut end = i + 2;
            while end < text.len() && is_identifier_continue(text[end]) {
                end += 1;
            }
            styled(&mut out, KEYWORD, &text[i..end]);
            i = end;
            line_start = false;
            continue;
        }
        if ch == b'\'' {
            let end = scan_quoted(text, i, false, true, false);
            styled(&mut out, STRING, &text[i..end]);
            i = end;
            line_start = false;
            continue;
        }
        if matches!(ch, b'"' | b'`') {
            let end = scan_quoted(text, i, false, true, false);
            styled(&mut out, TYPE, &text[i..end]);
            i = end;
            line_start = false;
            continue;
        }
        if ch == b'[' {
            let end = text[i + 1..]
                .iter()
                .position(|next| *next == b']')
                .map_or(text.len(), |offset| i + offset + 2);
            styled(&mut out, TYPE, &text[i..end]);
            i = end;
            line_start = false;
            continue;
        }
        if matches!(ch, b'?' | b':' | b'@' | b'$') {
            let end = scan_sql_parameter(text, i);
            styled(&mut out, SPECIAL, &text[i..end]);
            i = end;
            line_start = false;
            continue;
        }
        if ch.is_ascii_digit() || (ch == b'.' && text.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let end = scan_number(text, i);
            styled(&mut out, NUMBER, &text[i..end]);
            i = end;
            line_start = false;
            continue;
        }
        if is_identifier_start(ch) {
            let end = scan_identifier(text, i);
            let word = &text[i..end];
            if word.len() == 1
                && matches!(word.first().map(u8::to_ascii_lowercase), Some(b'x'))
                && text.get(end) == Some(&b'\'')
            {
                let string_end = scan_quoted(text, end, false, true, false);
                styled(&mut out, STRING, &text[i..string_end]);
                i = string_end;
            } else if sql_word_is(word, b"NULL")
                || sql_word_is(word, b"TRUE")
                || sql_word_is(word, b"FALSE")
            {
                styled(&mut out, NUMBER, word);
                i = end;
            } else if is_sql_keyword(word) {
                styled(&mut out, KEYWORD, word);
                i = end;
            } else if next_non_whitespace(text, end) == Some(b'(') {
                styled(&mut out, TYPE, word);
                i = end;
            } else {
                out.extend_from_slice(word);
                i = end;
            }
            line_start = false;
            continue;
        }
        let operator_len = matching_operator(text, i, OPERATORS);
        if operator_len > 0 {
            styled(&mut out, SPECIAL, &text[i..i + operator_len]);
            i += operator_len;
            line_start = false;
            continue;
        }
        out.push(ch);
        i += 1;
        line_start = false;
    }
    out
}

// C -------------------------------------------------------------------------

fn c_string_quote(text: &[u8], start: usize) -> Option<usize> {
    if matches!(text[start], b'\'' | b'"') {
        return Some(start);
    }
    if matches!(text[start], b'L' | b'u' | b'U')
        && matches!(text.get(start + 1), Some(b'\'') | Some(b'"'))
    {
        return Some(start + 1);
    }
    if text[start..].starts_with(b"u8") && matches!(text.get(start + 2), Some(b'\'') | Some(b'"')) {
        return Some(start + 2);
    }
    None
}

fn is_c_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"alignas"
            | b"alignof"
            | b"auto"
            | b"break"
            | b"case"
            | b"constexpr"
            | b"continue"
            | b"const"
            | b"default"
            | b"do"
            | b"else"
            | b"enum"
            | b"extern"
            | b"for"
            | b"goto"
            | b"if"
            | b"inline"
            | b"register"
            | b"restrict"
            | b"return"
            | b"sizeof"
            | b"static"
            | b"static_assert"
            | b"struct"
            | b"switch"
            | b"thread_local"
            | b"typedef"
            | b"typeof"
            | b"typeof_unqual"
            | b"union"
            | b"volatile"
            | b"while"
            | b"_Alignas"
            | b"_Alignof"
            | b"_Atomic"
            | b"_Generic"
            | b"_Noreturn"
            | b"_Static_assert"
            | b"_Thread_local"
    )
}

fn is_c_type(word: &[u8]) -> bool {
    matches!(
        word,
        b"bool"
            | b"char"
            | b"double"
            | b"float"
            | b"int"
            | b"long"
            | b"short"
            | b"signed"
            | b"unsigned"
            | b"void"
            | b"_BitInt"
            | b"_Bool"
            | b"_Complex"
            | b"_Decimal32"
            | b"_Decimal64"
            | b"_Decimal128"
            | b"_Imaginary"
    )
}

fn is_uppercase_macro(word: &[u8]) -> bool {
    word.len() > 1
        && word.iter().any(u8::is_ascii_alphabetic)
        && word.iter().all(|ch| !ch.is_ascii_lowercase())
}

fn highlight_c(text: &[u8]) -> Vec<u8> {
    const OPERATORS: &[&[u8]] = &[
        b">>=", b"<<=", b"...", b"->", b"++", b"--", b"&&", b"||", b"<=", b">=", b"==", b"!=",
        b"+=", b"-=", b"*=", b"/=", b"%=", b"&=", b"|=", b"^=", b"<<", b">>", b"+", b"-", b"*",
        b"/", b"%", b"&", b"|", b"^", b"~", b"!", b"<", b">", b"=", b"?", b":",
    ];

    let mut out = Vec::with_capacity(text.len() + 64);
    let mut i = 0;
    let mut line_start = true;
    let mut include_header = false;
    while i < text.len() {
        let ch = text[i];
        if ch.is_ascii_whitespace() {
            out.push(ch);
            line_start = ch == b'\n' || line_start;
            i += 1;
            continue;
        }
        if line_start && ch == b'#' {
            let start = i;
            i += 1;
            while i < text.len() && matches!(text[i], b' ' | b'\t') {
                i += 1;
            }
            let directive_start = i;
            while i < text.len() && is_identifier_continue(text[i]) {
                i += 1;
            }
            include_header = matches!(
                &text[directive_start..i],
                b"include" | b"include_next" | b"import"
            );
            styled(&mut out, KEYWORD, &text[start..i]);
            line_start = false;
            continue;
        }
        if include_header && ch == b'<' {
            let end = text[i + 1..]
                .iter()
                .position(|next| *next == b'>')
                .map_or(text.len(), |offset| i + offset + 2);
            styled(&mut out, STRING, &text[i..end]);
            i = end;
            include_header = false;
            continue;
        }
        if text[i..].starts_with(b"//") {
            let end = line_end(text, i);
            styled(&mut out, COMMENT, &text[i..end]);
            i = end;
            continue;
        }
        if text[i..].starts_with(b"/*") {
            let mut end = i + 2;
            while end + 1 < text.len() && !text[end..].starts_with(b"*/") {
                end += 1;
            }
            end = (end + 2).min(text.len());
            styled(&mut out, COMMENT, &text[i..end]);
            line_start = line_start || text[i..end].contains(&b'\n');
            i = end;
            continue;
        }
        if let Some(quote_start) = c_string_quote(text, i) {
            let end = scan_quoted(text, quote_start, true, false, false);
            styled(&mut out, STRING, &text[i..end]);
            i = end;
            line_start = false;
            continue;
        }
        if ch.is_ascii_digit() || (ch == b'.' && text.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let end = scan_number(text, i);
            styled(&mut out, NUMBER, &text[i..end]);
            i = end;
            line_start = false;
            continue;
        }
        if is_identifier_start(ch) {
            let end = scan_identifier(text, i);
            let word = &text[i..end];
            if is_c_type(word) {
                styled(&mut out, TYPE, word);
            } else if is_c_keyword(word) {
                styled(&mut out, KEYWORD, word);
            } else if matches!(word, b"NULL" | b"false" | b"nullptr" | b"true") {
                styled(&mut out, NUMBER, word);
            } else if next_non_whitespace(text, end) == Some(b'(') {
                styled(&mut out, TYPE, word);
            } else if is_uppercase_macro(word) {
                styled(&mut out, SPECIAL, word);
            } else {
                out.extend_from_slice(word);
            }
            i = end;
            line_start = false;
            continue;
        }
        let operator_len = matching_operator(text, i, OPERATORS);
        if operator_len > 0 {
            styled(&mut out, SPECIAL, &text[i..i + operator_len]);
            i += operator_len;
            line_start = false;
            continue;
        }
        out.push(ch);
        i += 1;
        line_start = false;
    }
    out
}
