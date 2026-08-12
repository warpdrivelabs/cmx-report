//! ast —— 报表公式的抽象语法树 + 词法 + 递归下降解析。
//!
//! 文法承 `cmx-biz/src/doc/formula.rs`（or→and→cmp→add→mul→unary→primary），
//! **primary 扩三样**：函数调用 `ident(args)`、单元格引用 `C5` / 区间 `C5:D10`、
//! 组织记号 `@current`/`@parent`。函数名解析期大写化（`qm(...)`==`QM(...)`）。
//!
//! 解析产物 `Node` 是纯数据、无求值——求值在 `eval.rs`，依赖解析在 `resolve.rs`。

/// 语法树节点。
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Num(rust_decimal::Decimal),
    Str(String),
    Bool(bool),
    Null,
    /// 裸标识符：字段引用（从 scope 取，缺失按 0，与 doc/formula.rs 一致）。
    Ident(String),
    /// 组织记号 `@current` / `@parent` / `@root`（参数解析用）。
    OrgRef(String),
    /// 单元格引用 `C5`（本表）。求值时经 resolve 回调解析。
    Cell(String),
    /// 单元格区间 `C5:D10`（本表）。展开成矩形内所有单元格。
    Range(String, String),
    Unary(String, Box<Node>),
    Binary(String, Box<Node>, Box<Node>),
    /// 函数调用（变参）。name 已大写化。
    Call(String, Vec<Node>),
}

/// 解析入口：表达式串 → AST。失败返回错误串。
pub fn parse(expr: &str) -> Result<Node, String> {
    let tokens = lex(expr)?;
    let mut p = Parser { tokens, pos: 0 };
    let node = p.parse_expr()?;
    if p.pos != p.tokens.len() {
        return Err(format!("表达式有多余 token @ {}", p.pos));
    }
    Ok(node)
}

// ─────────────────────── 词法 ───────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(rust_decimal::Decimal),
    Str(String),
    Ident(String),
    /// `@current` 等组织记号（去掉 @ 后的名字，小写化）。
    Org(String),
    Op(String),
    LParen,
    RParen,
    Comma,
    Colon,
    True,
    False,
    Null,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    use rust_decimal::Decimal;
    use std::str::FromStr;

    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out: Vec<Tok> = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            ':' => {
                out.push(Tok::Colon);
                i += 1;
            }
            '+' | '*' | '/' => {
                out.push(Tok::Op(c.to_string()));
                i += 1;
            }
            '-' => {
                // 负号：数字紧跟且处于表达式起始/运算符/逗号/左括号后 → 负数字面量
                // （期间参数 -1/-2/-12 必须词法上就是负数，才能被 as_num 取到整数偏移）。
                let prev_allows_unary = matches!(
                    out.last(),
                    None | Some(Tok::Op(_))
                        | Some(Tok::LParen)
                        | Some(Tok::Comma)
                        | Some(Tok::Colon)
                );
                if prev_allows_unary
                    && i + 1 < cs.len()
                    && (cs[i + 1].is_ascii_digit() || cs[i + 1] == '.')
                {
                    let mut buf = String::from("-");
                    i += 1;
                    while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                        buf.push(cs[i]);
                        i += 1;
                    }
                    let n = Decimal::from_str(&buf).map_err(|_| format!("非法数字 {buf}"))?;
                    out.push(Tok::Num(n));
                } else {
                    out.push(Tok::Op("-".into()));
                    i += 1;
                }
            }
            '>' | '<' | '=' | '!' => {
                if i + 1 < cs.len() && cs[i + 1] == '=' {
                    out.push(Tok::Op(format!("{c}=")));
                    i += 2;
                } else if c == '!' {
                    out.push(Tok::Op("!".into()));
                    i += 1;
                } else if c == '=' {
                    return Err("单个 = 非法（用 ==）".into());
                } else {
                    out.push(Tok::Op(c.to_string()));
                    i += 1;
                }
            }
            '&' | '|' => {
                if i + 1 < cs.len() && cs[i + 1] == c {
                    out.push(Tok::Op(format!("{c}{c}")));
                    i += 2;
                } else {
                    return Err(format!("非法运算符 {c}"));
                }
            }
            '@' => {
                // 组织记号 @current / @parent / @root
                i += 1;
                let mut buf = String::new();
                while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                    buf.push(cs[i]);
                    i += 1;
                }
                if buf.is_empty() {
                    return Err("@ 后需跟组织记号（如 @current）".into());
                }
                out.push(Tok::Org(buf.to_ascii_lowercase()));
            }
            '\'' | '"' => {
                let quote = c;
                i += 1;
                let mut buf = String::new();
                while i < cs.len() && cs[i] != quote {
                    buf.push(cs[i]);
                    i += 1;
                }
                if i >= cs.len() {
                    return Err("字符串未闭合".into());
                }
                i += 1;
                out.push(Tok::Str(buf));
            }
            _ if c.is_ascii_digit() || c == '.' => {
                let mut buf = String::new();
                while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                    buf.push(cs[i]);
                    i += 1;
                }
                let n = Decimal::from_str(&buf).map_err(|_| format!("非法数字 {buf}"))?;
                out.push(Tok::Num(n));
            }
            _ if c.is_alphabetic() || c == '_' => {
                let mut buf = String::new();
                while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_' || cs[i] == '.') {
                    buf.push(cs[i]);
                    i += 1;
                }
                match buf.to_ascii_lowercase().as_str() {
                    "true" => out.push(Tok::True),
                    "false" => out.push(Tok::False),
                    "null" => out.push(Tok::Null),
                    _ => out.push(Tok::Ident(buf)),
                }
            }
            _ => return Err(format!("非法字符 {c}")),
        }
    }
    Ok(out)
}

// ─────────────────────── 语法（递归下降） ───────────────────────

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Result<Node, String> {
        self.parse_or()
    }
    fn parse_or(&mut self) -> Result<Node, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Op(o)) if o == "||") {
            self.next();
            let right = self.parse_and()?;
            left = Node::Binary("||".into(), Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_and(&mut self) -> Result<Node, String> {
        let mut left = self.parse_cmp()?;
        while matches!(self.peek(), Some(Tok::Op(o)) if o == "&&") {
            self.next();
            let right = self.parse_cmp()?;
            left = Node::Binary("&&".into(), Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_cmp(&mut self) -> Result<Node, String> {
        let mut left = self.parse_add()?;
        while let Some(Tok::Op(o)) = self.peek() {
            if matches!(o.as_str(), ">" | "<" | ">=" | "<=" | "==" | "!=") {
                let op = o.clone();
                self.next();
                let right = self.parse_add()?;
                left = Node::Binary(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }
    fn parse_add(&mut self) -> Result<Node, String> {
        let mut left = self.parse_mul()?;
        while let Some(Tok::Op(o)) = self.peek() {
            if o == "+" || o == "-" {
                let op = o.clone();
                self.next();
                let right = self.parse_mul()?;
                left = Node::Binary(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }
    fn parse_mul(&mut self) -> Result<Node, String> {
        let mut left = self.parse_unary()?;
        while let Some(Tok::Op(o)) = self.peek() {
            if o == "*" || o == "/" {
                let op = o.clone();
                self.next();
                let right = self.parse_unary()?;
                left = Node::Binary(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }
    fn parse_unary(&mut self) -> Result<Node, String> {
        if let Some(Tok::Op(o)) = self.peek()
            && (o == "-" || o == "!")
        {
            let op = o.clone();
            self.next();
            let operand = self.parse_unary()?;
            return Ok(Node::Unary(op, Box::new(operand)));
        }
        self.parse_primary()
    }
    fn parse_primary(&mut self) -> Result<Node, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Node::Num(n)),
            Some(Tok::Str(s)) => Ok(Node::Str(s)),
            Some(Tok::Org(o)) => Ok(Node::OrgRef(o)),
            Some(Tok::True) => Ok(Node::Bool(true)),
            Some(Tok::False) => Ok(Node::Bool(false)),
            Some(Tok::Null) => Ok(Node::Null),
            Some(Tok::LParen) => {
                let e = self.parse_expr()?;
                match self.next() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err("缺少 )".into()),
                }
            }
            Some(Tok::Ident(name)) => {
                if matches!(self.peek(), Some(Tok::LParen)) {
                    // 函数调用
                    self.next(); // (
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            args.push(self.parse_expr()?);
                            match self.peek() {
                                Some(Tok::Comma) => {
                                    self.next();
                                }
                                _ => break,
                            }
                        }
                    }
                    match self.next() {
                        Some(Tok::RParen) => Ok(Node::Call(name.to_ascii_uppercase(), args)),
                        _ => Err("函数缺少 )".into()),
                    }
                } else if is_cell_ref(&name) {
                    // 单元格引用或区间：C5 或 C5:D10
                    if matches!(self.peek(), Some(Tok::Colon)) {
                        self.next(); // :
                        match self.next() {
                            Some(Tok::Ident(end)) if is_cell_ref(&end) => Ok(Node::Range(
                                name.to_ascii_uppercase(),
                                end.to_ascii_uppercase(),
                            )),
                            other => Err(format!("区间右端非法单元格: {other:?}")),
                        }
                    } else {
                        Ok(Node::Cell(name.to_ascii_uppercase()))
                    }
                } else {
                    // 裸字段引用
                    Ok(Node::Ident(name))
                }
            }
            other => Err(format!("意外 token: {other:?}")),
        }
    }
}

/// 是否形如 A1 单元格引用：≥1 个字母后跟 ≥1 个数字，无其它字符（不含 `.`）。
pub fn is_cell_ref(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == 0 || i == bytes.len() {
        return false;
    }
    let mut has_digit = false;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            has_digit = true;
            i += 1;
        } else {
            return false;
        }
    }
    has_digit
}

/// 拆分 A1 引用为 (列字母大写, 行号)。非法返回 None。
pub fn split_cell_ref(s: &str) -> Option<(String, u32)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == 0 || i == bytes.len() {
        return None;
    }
    let col = s[..i].to_ascii_uppercase();
    let row: u32 = s[i..].parse().ok()?;
    Some((col, row))
}

/// 列字母 → 0 基列号（A→0, Z→25, AA→26）。
pub fn col_to_index(col: &str) -> u32 {
    let mut n: u32 = 0;
    for b in col.bytes() {
        if b.is_ascii_alphabetic() {
            n = n * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32;
        }
    }
    n.saturating_sub(1)
}

/// 0 基列号 → 列字母（0→A）。
pub fn index_to_col(mut idx: u32) -> String {
    let mut s = String::new();
    idx += 1;
    while idx > 0 {
        let r = (idx - 1) % 26;
        s.insert(0, (b'A' + r as u8) as char);
        idx = (idx - 1) / 26;
    }
    s
}

/// 展开区间 `C5:D10` 为逐个单元格引用（按行优先），供 SUM(REF(...)) / SUM(C5:D10)。
pub fn expand_range(start: &str, end: &str) -> Vec<String> {
    let (Some((c1, r1)), Some((c2, r2))) = (split_cell_ref(start), split_cell_ref(end)) else {
        return Vec::new();
    };
    let (ci1, ci2) = (col_to_index(&c1), col_to_index(&c2));
    let (lo_c, hi_c) = (ci1.min(ci2), ci1.max(ci2));
    let (lo_r, hi_r) = (r1.min(r2), r1.max(r2));
    let mut out = Vec::new();
    for r in lo_r..=hi_r {
        for c in lo_c..=hi_c {
            out.push(format!("{}{}", index_to_col(c), r));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[test]
    fn cell_ref_recognition() {
        assert!(is_cell_ref("C5"));
        assert!(is_cell_ref("AA100"));
        assert!(!is_cell_ref("C"));
        assert!(!is_cell_ref("5"));
        assert!(!is_cell_ref("C5D")); // 字母-数字-字母 非法
        assert!(!is_cell_ref("a.b")); // 含点 → 字段
    }

    #[test]
    fn negative_period_literal() {
        // QM(-1, ...) 的 -1 必须是负数字面量，不是二元减
        let n = parse("QM(-1)").unwrap();
        match n {
            Node::Call(name, args) => {
                assert_eq!(name, "QM");
                assert_eq!(args[0], Node::Num(Decimal::from(-1)));
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn subtraction_still_works() {
        // C5 - C6 是二元减，不是 C5 后跟负数
        let n = parse("C5 - C6").unwrap();
        assert!(matches!(n, Node::Binary(op, _, _) if op == "-"));
    }

    #[test]
    fn org_ref_and_string_arg() {
        let n = parse("QM(0, @current, '1001')").unwrap();
        match n {
            Node::Call(_, args) => {
                assert_eq!(args[0], Node::Num(Decimal::ZERO));
                assert_eq!(args[1], Node::OrgRef("current".into()));
                assert_eq!(args[2], Node::Str("1001".into()));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn range_and_col_math() {
        let n = parse("SUM(D3:D7)").unwrap();
        match n {
            Node::Call(_, args) => assert_eq!(args[0], Node::Range("D3".into(), "D7".into())),
            _ => panic!(),
        }
        assert_eq!(col_to_index("A"), 0);
        assert_eq!(col_to_index("AA"), 26);
        assert_eq!(index_to_col(0), "A");
        assert_eq!(index_to_col(26), "AA");
        assert_eq!(expand_range("C5", "D6"), vec!["C5", "D5", "C6", "D6"]);
    }
}
