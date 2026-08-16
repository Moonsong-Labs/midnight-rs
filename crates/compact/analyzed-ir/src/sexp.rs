//! A reader for the subset of Scheme datum syntax the artifact uses:
//! symbols, strings, exact integers, `#t`/`#f`, `#vu8(...)` bytevectors,
//! proper lists, dotted pairs, and the `'x` quote shorthand.

use num_bigint::BigInt;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sexp {
    Sym(String),
    Str(String),
    Int(BigInt),
    Bool(bool),
    Bytes(Vec<u8>),
    List(Vec<Sexp>),
    /// A dotted pair whose cdr is not a list, `(a . b)`. A dotted pair whose
    /// cdr is a list prints as a plain list, so it arrives as `List`.
    Pair(Box<Sexp>, Box<Sexp>),
}

impl Sexp {
    pub fn as_sym(&self) -> Option<&str> {
        match self {
            Sexp::Sym(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Sexp]> {
        match self {
            Sexp::List(l) => Some(l),
            _ => None,
        }
    }
    /// The head symbol of a non-empty list.
    pub fn head(&self) -> Option<&str> {
        self.as_list()
            .and_then(|l| l.first())
            .and_then(|h| h.as_sym())
    }
}

impl fmt::Display for Sexp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Sexp::Sym(s) => write!(f, "{s}"),
            Sexp::Str(s) => write!(f, "{s:?}"),
            Sexp::Int(i) => write!(f, "{i}"),
            Sexp::Bool(b) => write!(f, "{}", if *b { "#t" } else { "#f" }),
            Sexp::Bytes(b) => write!(f, "#vu8(..{} bytes..)", b.len()),
            Sexp::List(l) => {
                write!(f, "(")?;
                for (i, e) in l.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{e}")?;
                }
                write!(f, ")")
            }
            Sexp::Pair(a, b) => write!(f, "({a} . {b})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    pub at: usize,
    pub message: String,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "read error at byte {}: {}", self.at, self.message)
    }
}

impl std::error::Error for ReadError {}

pub struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(src: &'a str) -> Self {
        Reader {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn err(&self, message: impl Into<String>) -> ReadError {
        ReadError {
            at: self.pos,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.pos += 1;
                }
                b';' => {
                    while let Some(c) = self.peek() {
                        self.pos += 1;
                        if c == b'\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    /// Read one datum. `[` and `]` are accepted as parenthesis synonyms.
    pub fn read(&mut self) -> Result<Sexp, ReadError> {
        self.skip_ws();
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'(') | Some(b'[') => {
                self.pos += 1;
                self.read_list()
            }
            Some(b')') | Some(b']') => Err(self.err("unexpected closing parenthesis")),
            Some(b'\'') => {
                self.pos += 1;
                let quoted = self.read()?;
                Ok(Sexp::List(vec![Sexp::Sym("quote".into()), quoted]))
            }
            Some(b'"') => self.read_string(),
            Some(b'#') => self.read_hash(),
            Some(_) => self.read_atom(),
        }
    }

    fn read_list(&mut self) -> Result<Sexp, ReadError> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(self.err("unterminated list")),
                Some(b')') | Some(b']') => {
                    self.pos += 1;
                    return Ok(Sexp::List(items));
                }
                Some(b'.') if self.lone_dot() => {
                    self.pos += 1;
                    let tail = self.read()?;
                    self.skip_ws();
                    match self.bump() {
                        Some(b')') | Some(b']') => {}
                        _ => return Err(self.err("expected ) after dotted tail")),
                    }
                    // (a . (b c)) never prints; a dotted tail here is an atom.
                    if items.len() != 1 {
                        return Err(self.err("dotted pair with more than one car"));
                    }
                    let car = items.pop().expect("one item");
                    return Ok(Sexp::Pair(Box::new(car), Box::new(tail)));
                }
                Some(_) => items.push(self.read()?),
            }
        }
    }

    /// A `.` is the pair dot only when it stands alone; symbols like
    /// `%tmp.2` contain dots inside one token.
    fn lone_dot(&self) -> bool {
        matches!(
            self.src.get(self.pos + 1),
            None | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'(') | Some(b')')
        )
    }

    fn read_string(&mut self) -> Result<Sexp, ReadError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => return Ok(Sexp::Str(out)),
                Some(b'\\') => match self.bump() {
                    None => return Err(self.err("unterminated escape")),
                    Some(b'n') => out.push('\n'),
                    Some(b't') => out.push('\t'),
                    Some(c) => out.push(c as char),
                },
                Some(c) => out.push(c as char),
            }
        }
    }

    fn read_hash(&mut self) -> Result<Sexp, ReadError> {
        let rest = &self.src[self.pos..];
        if rest.starts_with(b"#t") {
            self.pos += 2;
            Ok(Sexp::Bool(true))
        } else if rest.starts_with(b"#f") {
            self.pos += 2;
            Ok(Sexp::Bool(false))
        } else if rest.starts_with(b"#vu8(") {
            self.pos += 5;
            let mut bytes = Vec::new();
            loop {
                self.skip_ws();
                match self.peek() {
                    None => return Err(self.err("unterminated bytevector")),
                    Some(b')') => {
                        self.pos += 1;
                        return Ok(Sexp::Bytes(bytes));
                    }
                    Some(_) => match self.read_atom()? {
                        Sexp::Int(i) => {
                            let (_, digits) = i.to_u64_digits();
                            match (digits.len(), i.sign()) {
                                (0, _) => bytes.push(0),
                                (1, num_bigint::Sign::Plus) if digits[0] <= 255 => {
                                    bytes.push(digits[0] as u8)
                                }
                                _ => return Err(self.err("bytevector element out of range")),
                            }
                        }
                        _ => return Err(self.err("non-integer in bytevector")),
                    },
                }
            }
        } else {
            Err(self.err("unrecognized # syntax"))
        }
    }

    fn read_atom(&mut self) -> Result<Sexp, ReadError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b'[' | b']' | b'"' | b';' => break,
                _ => self.pos += 1,
            }
        }
        if start == self.pos {
            return Err(self.err("empty token"));
        }
        let tok = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|_| self.err("non-utf8 token"))?;
        let numeric = {
            let t = tok.strip_prefix('-').unwrap_or(tok);
            !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
        };
        if numeric {
            Ok(Sexp::Int(
                tok.parse::<BigInt>().map_err(|e| self.err(e.to_string()))?,
            ))
        } else {
            Ok(Sexp::Sym(tok.to_string()))
        }
    }
}

/// Read the artifact's single top-level datum.
pub fn read_one(src: &str) -> Result<Sexp, ReadError> {
    let mut r = Reader::new(src);
    let datum = r.read()?;
    r.skip_ws();
    if r.peek().is_some() {
        return Err(r.err("trailing content after the datum"));
    }
    Ok(datum)
}
