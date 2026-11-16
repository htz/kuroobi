//! GGS's match-request formulas (finger's `decline`), evaluated the way the server does
//! (`Service/GameLib/src/VC_Formula.C`), so a request it would turn away is caught before it is
//! sent. Values only the server knows (`rnd`, `playing`) are unknown here, and a formula whose
//! result hangs on them is left to the server.

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ty {
    Bool,
    Int,
    Real,
    Color,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Val {
    Bool(bool),
    Int(i64),
    Real(f64),
    Color(char),
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug)]
enum Node {
    Lit(Val),
    Var(&'static str),
    /// `md(x)`, `od(x)`, `rnd(x)`, `playing(x)`.
    Call(&'static str, Box<Expr>),
    Neg(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
}

#[derive(Debug)]
struct Expr {
    node: Node,
    ty: Ty,
    /// Byte range in the formula text, to quote a condition as its owner wrote it.
    span: (usize, usize),
}

const VARS: [(&str, Ty); 31] = [
    ("synchro", Ty::Bool),
    ("saved", Ty::Bool),
    ("stored", Ty::Int),
    ("rated", Ty::Bool),
    ("rand", Ty::Bool),
    ("komi", Ty::Bool),
    ("anti", Ty::Bool),
    ("discs", Ty::Int),
    ("size", Ty::Int),
    ("mc", Ty::Color),
    ("oc", Ty::Color),
    ("mt1", Ty::Int),
    ("mm1", Ty::Int),
    ("ml1", Ty::Bool),
    ("mt2", Ty::Int),
    ("mm2", Ty::Int),
    ("ma2", Ty::Bool),
    ("mt3", Ty::Int),
    ("mm3", Ty::Int),
    ("ma3", Ty::Bool),
    ("ot1", Ty::Int),
    ("om1", Ty::Int),
    ("ol1", Ty::Bool),
    ("ot2", Ty::Int),
    ("om2", Ty::Int),
    ("oa2", Ty::Bool),
    ("ot3", Ty::Int),
    ("om3", Ty::Int),
    ("oa3", Ty::Bool),
    ("mr", Ty::Real),
    ("or", Ty::Real),
];

const CALLS: [(&str, Ty, Ty); 4] = [
    ("md(", Ty::Real, Ty::Real),
    ("od(", Ty::Real, Ty::Real),
    ("rnd(", Ty::Int, Ty::Int),
    ("playing(", Ty::Int, Ty::Bool),
];

struct Parser<'a> {
    s: &'a str,
    at: usize,
}

type PResult = Result<Expr, String>;

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.at..]
    }

    fn skip_ws(&mut self) {
        while self.rest().starts_with(char::is_whitespace) {
            self.at += self.rest().chars().next().map_or(1, char::len_utf8);
        }
    }

    /// The server matches keywords as plain prefixes and then skips the blanks after them.
    fn token(&mut self, kw: &str) -> bool {
        if self.rest().starts_with(kw) {
            self.at += kw.len();
            self.skip_ws();
            true
        } else {
            false
        }
    }

    fn bin(op: Op, ty: Ty, a: Expr, b: Expr) -> Expr {
        let span = (a.span.0, b.span.1);
        Expr {
            node: Node::Bin(op, Box::new(a), Box::new(b)),
            ty,
            span,
        }
    }

    // Like the server, `|` and `&` share one level and bind to the right, as do `+ -` and `* /`.
    fn e1(&mut self) -> PResult {
        let a = self.e2()?;
        for (kw, op) in [("|", Op::Or), ("&", Op::And)] {
            if self.token(kw) {
                if a.ty != Ty::Bool {
                    return Err(format!("{kw} needs BOOL"));
                }
                let b = self.e1()?;
                return Ok(Self::bin(op, Ty::Bool, a, b));
            }
        }
        Ok(a)
    }

    fn e2(&mut self) -> PResult {
        let a = self.e3()?;
        for (kw, op) in [
            ("==", Op::Eq),
            ("!=", Op::Ne),
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("<", Op::Lt),
            (">", Op::Gt),
        ] {
            if self.token(kw) {
                if !matches!(op, Op::Eq | Op::Ne) && !matches!(a.ty, Ty::Int | Ty::Real) {
                    return Err(format!("{kw} needs a number"));
                }
                let b = self.e3()?;
                return Ok(Self::bin(op, Ty::Bool, a, b));
            }
        }
        Ok(a)
    }

    fn e3(&mut self) -> PResult {
        let a = self.e4()?;
        for (kw, op) in [("+", Op::Add), ("-", Op::Sub)] {
            if self.token(kw) {
                let ty = a.ty;
                let b = self.e3()?;
                return Ok(Self::bin(op, ty, a, b));
            }
        }
        Ok(a)
    }

    fn e4(&mut self) -> PResult {
        let a = self.e5()?;
        for (kw, op) in [("*", Op::Mul), ("/", Op::Div), ("%", Op::Mod)] {
            if self.token(kw) {
                let ty = a.ty;
                let b = self.e4()?;
                return Ok(Self::bin(op, ty, a, b));
            }
        }
        Ok(a)
    }

    fn e5(&mut self) -> PResult {
        let start = self.at;
        for (name, ty) in VARS {
            if self.token(name) {
                return Ok(Expr {
                    node: Node::Var(name),
                    ty,
                    span: (start, start + name.len()),
                });
            }
        }
        for (name, ty, _) in CALLS {
            if self.token(name) {
                let arg = self.e3()?;
                if !self.rest().starts_with(')') {
                    return Err("')' expected".into());
                }
                let end = self.at + 1;
                self.token(")");
                return Ok(Expr {
                    node: Node::Call(name.trim_end_matches('('), Box::new(arg)),
                    ty,
                    span: (start, end),
                });
            }
        }
        if self.token("+") {
            return self.e5();
        }
        if self.token("-") || self.token("!") {
            let a = self.e5()?;
            let span = (start, a.span.1);
            let ty = a.ty;
            return Ok(Expr {
                node: Node::Neg(Box::new(a)),
                ty,
                span,
            });
        }
        if self.token("(") {
            let a = self.e1()?;
            if !self.rest().starts_with(')') {
                return Err("')' expected".into());
            }
            let end = self.at + 1;
            self.token(")");
            return Ok(Expr {
                span: (start, end),
                ..a
            });
        }
        self.literal(start)
    }

    fn literal(&mut self, start: usize) -> PResult {
        let r = self.rest();
        let lit = |p: &mut Self, len: usize, v: Val, ty: Ty| {
            p.at += len;
            let end = p.at;
            p.skip_ws();
            Ok(Expr {
                node: Node::Lit(v),
                ty,
                span: (start, end),
            })
        };
        match r.chars().next() {
            Some(c @ ('T' | 'F')) => return lit(self, 1, Val::Bool(c == 'T'), Ty::Bool),
            Some(c @ ('B' | 'W' | '?')) => return lit(self, 1, Val::Color(c), Ty::Color),
            _ => {}
        }
        let len = r
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(r.len());
        let num = &r[..len];
        if num.contains('.') {
            let v: f64 = num.parse().map_err(|_| format!("bad number {num}"))?;
            lit(self, len, Val::Real(v), Ty::Real)
        } else if !num.is_empty() {
            let v: i64 = num.parse().map_err(|_| format!("bad number {num}"))?;
            lit(self, len, Val::Int(v), Ty::Int)
        } else {
            Err(format!("unexpected: {r}"))
        }
    }
}

/// One player's side of a GGS clock (`GAME_Clock`): seconds and the move counts and flags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    pub ini: i64,
    pub ini_moves: i64,
    pub ini_loss: bool,
    pub inc: i64,
    pub inc_moves: i64,
    pub inc_add: bool,
    pub ext: i64,
    pub ext_moves: i64,
    pub ext_add: bool,
}

/// `h:m:s`, `m:s`, or a bare number of minutes, as `HHMMSS::parse` with `Min` reads it.
fn secs(t: &str) -> Option<i64> {
    let t = t.split('.').next()?;
    let parts: Vec<i64> = t
        .split(':')
        .map(|p| p.parse::<i64>().ok())
        .collect::<Option<_>>()?;
    match parts[..] {
        [m] => Some(m * 60),
        [m, s] => Some(m * 60 + s),
        [h, m, s] => Some(h * 3600 + m * 60 + s),
        _ => None,
    }
}

impl Clock {
    /// `[ini[,[N]m1]]/[inc[,[N]m2]]/[ext[,[N]m3]]` over Othello's default `15:00//2:00`.
    pub fn parse(s: &str) -> Option<Clock> {
        let mut c = Clock {
            ini: 900,
            ini_moves: 0,
            ini_loss: true,
            inc: 0,
            inc_moves: 1,
            inc_add: true,
            ext: 120,
            ext_moves: 0,
            ext_add: true,
        };
        let mut parts = s.splitn(3, '/');
        let field = |part: Option<&str>, t: &mut i64, m: &mut i64, flag: &mut bool| -> Option<()> {
            let Some(part) = part else { return Some(()) };
            let (tt, mm) = part.split_once(',').unwrap_or((part, ""));
            if !tt.is_empty() {
                *t = secs(tt)?;
            }
            if !mm.is_empty() {
                let mm = match mm.strip_prefix(['N', 'n']) {
                    Some(rest) => {
                        *flag = false;
                        rest
                    }
                    None => mm,
                };
                *m = mm.parse().ok()?;
            }
            Some(())
        };
        field(parts.next(), &mut c.ini, &mut c.ini_moves, &mut c.ini_loss)?;
        field(parts.next(), &mut c.inc, &mut c.inc_moves, &mut c.inc_add)?;
        field(parts.next(), &mut c.ext, &mut c.ext_moves, &mut c.ext_add)?;
        Some(c)
    }
}

/// A request as the formula's owner (the one asked) sees it.
#[derive(Clone, Debug)]
pub struct Facts {
    pub synchro: bool,
    pub rand: bool,
    pub komi: bool,
    pub anti: bool,
    /// Random discs; -1 when the game is not random.
    pub discs: i64,
    pub size: i64,
    pub saved: bool,
    pub stored: i64,
    pub rated: bool,
    /// The asker's colour wish: 'B', 'W' or '?'.
    pub asker_color: char,
    pub asker_clock: Clock,
    pub owner_clock: Clock,
    /// Rating and deviation in the request's pool, when known.
    pub asker_rating: Option<(f64, f64)>,
    pub owner_rating: Option<(f64, f64)>,
}

impl Facts {
    /// The request type (`s8r16`, `8`, `8b`, `k8`, ...) as `MatchType::parse` reads it.
    pub fn from_request(gtype: &str, time: &str) -> Option<Facts> {
        let g = gtype.trim();
        let (synchro, g) = match g.strip_prefix(['s', 'S']) {
            Some(r) => (true, r),
            None => (false, g),
        };
        let digits = g.find(|c: char| !c.is_ascii_digit()).unwrap_or(g.len());
        let size: i64 = g[..digits].parse().ok()?;
        let mut rest = &g[digits..];
        let (mut rand, mut komi, mut anti, mut discs, mut color) = (false, false, false, -1, '?');
        while let Some(c) = rest.chars().next() {
            rest = &rest[c.len_utf8()..];
            match c.to_ascii_uppercase() {
                'K' => komi = true,
                'A' => anti = true,
                'B' => color = 'B',
                'W' => color = 'W',
                'R' => {
                    rand = true;
                    let n = rest
                        .find(|c: char| !c.is_ascii_digit())
                        .unwrap_or(rest.len());
                    discs = rest[..n].parse().unwrap_or(0);
                    rest = &rest[n..];
                }
                _ => return None,
            }
        }
        let clock = Clock::parse(time)?;
        Some(Facts {
            synchro,
            rand,
            komi,
            anti,
            discs,
            size,
            saved: false,
            stored: 0,
            rated: false,
            asker_color: color,
            asker_clock: clock,
            owner_clock: clock,
            asker_rating: None,
            owner_rating: None,
        })
    }

    /// The rating pool the request is rated in (`MatchType::key`).
    pub fn pool(&self) -> String {
        format!(
            "{}{}{}",
            self.size,
            if self.rand { "r" } else { "" },
            if self.anti { "a" } else { "" }
        )
    }

    fn var(&self, name: &str) -> Val {
        let (o, a) = (&self.owner_clock, &self.asker_clock);
        let flip = |c: char| match c {
            'B' => 'W',
            'W' => 'B',
            c => c,
        };
        match name {
            "synchro" => Val::Bool(self.synchro),
            "saved" => Val::Bool(self.saved),
            "stored" => Val::Int(self.stored),
            "rated" => Val::Bool(self.rated),
            "rand" => Val::Bool(self.rand),
            "komi" => Val::Bool(self.komi),
            "anti" => Val::Bool(self.anti),
            "discs" => Val::Int(self.discs),
            "size" => Val::Int(self.size),
            "mc" => Val::Color(flip(self.asker_color)),
            "oc" => Val::Color(self.asker_color),
            "mt1" => Val::Int(o.ini),
            "mm1" => Val::Int(o.ini_moves),
            "ml1" => Val::Bool(o.ini_loss),
            "mt2" => Val::Int(o.inc),
            "mm2" => Val::Int(o.inc_moves),
            "ma2" => Val::Bool(o.inc_add),
            "mt3" => Val::Int(o.ext),
            "mm3" => Val::Int(o.ext_moves),
            "ma3" => Val::Bool(o.ext_add),
            "ot1" => Val::Int(a.ini),
            "om1" => Val::Int(a.ini_moves),
            "ol1" => Val::Bool(a.ini_loss),
            "ot2" => Val::Int(a.inc),
            "om2" => Val::Int(a.inc_moves),
            "oa2" => Val::Bool(a.inc_add),
            "ot3" => Val::Int(a.ext),
            "om3" => Val::Int(a.ext_moves),
            "oa3" => Val::Bool(a.ext_add),
            "mr" => self.owner_rating.map_or(Val::Unknown, |r| Val::Real(r.0)),
            "or" => self.asker_rating.map_or(Val::Unknown, |r| Val::Real(r.0)),
            _ => Val::Unknown,
        }
    }

    /// `md(x)` / `od(x)`: the rating change of the owner (or the asker) if the owner scored `x`.
    fn delta(&self, owner: bool, result: f64) -> Val {
        if !self.rated {
            return Val::Real(0.0);
        }
        let (Some(me), Some(them)) = (
            if owner {
                self.owner_rating
            } else {
                self.asker_rating
            },
            if owner {
                self.asker_rating
            } else {
                self.owner_rating
            },
        ) else {
            return Val::Unknown;
        };
        let score = othello_score(result);
        Val::Real(glicko_new(score, me, them) - me.0)
    }
}

/// `RegularBoardGame::score` for Othello: a disc result squashed into 0..1.
fn othello_score(result: f64) -> f64 {
    const K: f64 = 0.75;
    0.5 + result * K * 0.5 / (1.0 + result.abs() * K)
}

/// The first player's new rating from `VC_Rating::rate`.
fn glicko_new(score: f64, (r1, d1): (f64, f64), (r2, d2): (f64, f64)) -> f64 {
    const P: f64 = 0.000_010_072_415_617_567_8;
    const Q: f64 = 0.005_756_462_732_485_114;
    const MIN_K: f64 = 16.0;
    let f = 1.0 / (1.0 + P * d2 * d2).sqrt();
    let e = 1.0 / (1.0 + 10f64.powf(-(r1 - r2) * f / 400.0));
    let k = 1.0 / (d1 * d1) + Q * Q * f * f * e * (1.0 - e);
    let kk = (Q * f / k).max(MIN_K);
    r1 + 2.0 * kk * (score - e)
}

fn as_f(v: Val) -> Option<f64> {
    match v {
        Val::Int(i) => Some(i as f64),
        Val::Real(r) => Some(r),
        _ => None,
    }
}

fn eval(e: &Expr, f: &Facts) -> Val {
    match &e.node {
        Node::Lit(v) => *v,
        Node::Var(n) => f.var(n),
        Node::Call(name, arg) => match (*name, eval(arg, f)) {
            ("md", v) => as_f(v).map_or(Val::Unknown, |x| f.delta(true, x)),
            ("od", v) => as_f(v).map_or(Val::Unknown, |x| f.delta(false, x)),
            _ => Val::Unknown,
        },
        Node::Neg(a) => match eval(a, f) {
            Val::Bool(b) => Val::Bool(!b),
            Val::Int(i) => Val::Int(-i),
            Val::Real(r) => Val::Real(-r),
            _ => Val::Unknown,
        },
        Node::Bin(op, a, b) => {
            let (x, y) = (eval(a, f), eval(b, f));
            match op {
                Op::Or => match (x, y) {
                    (Val::Bool(true), _) | (_, Val::Bool(true)) => Val::Bool(true),
                    (Val::Bool(false), Val::Bool(false)) => Val::Bool(false),
                    _ => Val::Unknown,
                },
                Op::And => match (x, y) {
                    (Val::Bool(false), _) | (_, Val::Bool(false)) => Val::Bool(false),
                    (Val::Bool(true), Val::Bool(true)) => Val::Bool(true),
                    _ => Val::Unknown,
                },
                Op::Eq | Op::Ne => {
                    let same = match (x, y) {
                        (Val::Bool(p), Val::Bool(q)) => Some(p == q),
                        (Val::Color(p), Val::Color(q)) => Some(p == q),
                        _ => as_f(x).zip(as_f(y)).map(|(p, q)| p == q),
                    };
                    same.map_or(Val::Unknown, |s| Val::Bool(s == (*op == Op::Eq)))
                }
                Op::Lt | Op::Le | Op::Gt | Op::Ge => match as_f(x).zip(as_f(y)) {
                    Some((p, q)) => Val::Bool(match op {
                        Op::Lt => p < q,
                        Op::Le => p <= q,
                        Op::Gt => p > q,
                        _ => p >= q,
                    }),
                    None => Val::Unknown,
                },
                Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod => arith(*op, e.ty, x, y),
            }
        }
    }
}

fn arith(op: Op, ty: Ty, x: Val, y: Val) -> Val {
    if ty == Ty::Int {
        let (Val::Int(p), Val::Int(q)) = (x, y) else {
            return Val::Unknown;
        };
        // The server answers 0 for a division by zero.
        Val::Int(match op {
            Op::Add => p + q,
            Op::Sub => p - q,
            Op::Mul => p * q,
            Op::Div => p.checked_div(q).unwrap_or(0),
            _ => p.checked_rem(q).unwrap_or(0),
        })
    } else {
        let Some((p, q)) = as_f(x).zip(as_f(y)) else {
            return Val::Unknown;
        };
        Val::Real(match op {
            Op::Add => p + q,
            Op::Sub => p - q,
            Op::Mul => p * q,
            Op::Div if q != 0.0 => p / q,
            _ => 0.0,
        })
    }
}

/// A comparison, read as "the request is turned away when `var <cmp> limit`".
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Cmp {
    fn of(op: Op) -> Option<Cmp> {
        Some(match op {
            Op::Lt => Cmp::Lt,
            Op::Le => Cmp::Le,
            Op::Gt => Cmp::Gt,
            Op::Ge => Cmp::Ge,
            Op::Eq => Cmp::Eq,
            Op::Ne => Cmp::Ne,
            _ => return None,
        })
    }

    /// `a < b` read from `b`'s side, as `b > a`.
    fn flip(self) -> Cmp {
        match self {
            Cmp::Lt => Cmp::Gt,
            Cmp::Le => Cmp::Ge,
            Cmp::Gt => Cmp::Lt,
            Cmp::Ge => Cmp::Le,
            c => c,
        }
    }
}

/// What a condition that turned the request away is about, so it can be said in words.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A yes/no property of the request; turned away when it is `when`.
    Flag { var: &'static str, when: bool },
    /// A number of the request; turned away when `var <cmp> limit`.
    Number {
        var: &'static str,
        cmp: Cmp,
        limit: f64,
    },
    /// Both players' clocks must match in `var` (`mt1 != ot1`).
    Same { var: &'static str },
    /// The owner's (`mc`) or the asker's (`oc`) colour; turned away when `var <cmp> color`.
    Color {
        var: &'static str,
        cmp: Cmp,
        color: char,
    },
    /// The owner's (`md`) or the asker's (`od`) rating change if the owner scored `result`;
    /// turned away when it is `<cmp> limit`.
    Delta {
        owner: bool,
        result: f64,
        cmp: Cmp,
        limit: f64,
    },
    /// Anything more involved than one variable against a constant.
    Other,
}

/// The opponent's formula laid out for reading: groups that need any one (`|`) or all (`&`) of
/// their parts, down to single conditions, each marked whether it holds for this request.
#[derive(Clone, Debug, PartialEq)]
pub enum Outline {
    Group {
        any: bool,
        hit: bool,
        kids: Vec<Outline>,
    },
    Cond {
        shape: Shape,
        hit: bool,
    },
}

impl Outline {
    pub fn hit(&self) -> bool {
        match self {
            Outline::Group { hit, .. } | Outline::Cond { hit, .. } => *hit,
        }
    }
}

fn lit_num(e: &Expr) -> Option<f64> {
    match &e.node {
        Node::Lit(v) => as_f(*v),
        Node::Neg(a) => lit_num(a).map(|x| -x),
        _ => None,
    }
}

fn shape_of(e: &Expr) -> Shape {
    match &e.node {
        Node::Var(v) if e.ty == Ty::Bool => Shape::Flag { var: v, when: true },
        Node::Neg(a) => match &a.node {
            Node::Var(v) if a.ty == Ty::Bool => Shape::Flag {
                var: v,
                when: false,
            },
            _ => Shape::Other,
        },
        Node::Bin(op, a, b) => {
            let Some(cmp) = Cmp::of(*op) else {
                return Shape::Other;
            };
            // The variable on the left: `16 > discs` reads as `discs < 16`.
            let (x, y, cmp) = match (&a.node, &b.node) {
                (Node::Var(_) | Node::Call(..), _) => (a, b, cmp),
                (_, Node::Var(_) | Node::Call(..)) => (b, a, cmp.flip()),
                _ => return Shape::Other,
            };
            match (&x.node, &y.node) {
                (Node::Var(v), Node::Var(w)) if cmp == Cmp::Ne && v[1..] == w[1..] => {
                    Shape::Same { var: v }
                }
                (Node::Var(v), Node::Lit(Val::Bool(t))) if matches!(cmp, Cmp::Eq | Cmp::Ne) => {
                    Shape::Flag {
                        var: v,
                        when: *t == (cmp == Cmp::Eq),
                    }
                }
                (Node::Var(v), Node::Lit(Val::Color(c))) => Shape::Color {
                    var: v,
                    cmp,
                    color: *c,
                },
                (Node::Var(v), _) => match lit_num(y) {
                    Some(limit) => Shape::Number { var: v, cmp, limit },
                    None => Shape::Other,
                },
                (Node::Call(name @ ("md" | "od"), arg), _) => match (lit_num(arg), lit_num(y)) {
                    (Some(result), Some(limit)) => Shape::Delta {
                        owner: *name == "md",
                        result,
                        cmp,
                        limit,
                    },
                    _ => Shape::Other,
                },
                _ => Shape::Other,
            }
        }
        _ => Shape::Other,
    }
}

/// The same condition read the other way round, for a `!` pushed down onto it.
fn negate(shape: Shape) -> Shape {
    let flip = |c: Cmp| match c {
        Cmp::Lt => Cmp::Ge,
        Cmp::Le => Cmp::Gt,
        Cmp::Gt => Cmp::Le,
        Cmp::Ge => Cmp::Lt,
        Cmp::Eq => Cmp::Ne,
        Cmp::Ne => Cmp::Eq,
    };
    match shape {
        Shape::Flag { var, when } => Shape::Flag { var, when: !when },
        Shape::Number { var, cmp, limit } => Shape::Number {
            var,
            cmp: flip(cmp),
            limit,
        },
        Shape::Color { var, cmp, color } => Shape::Color {
            var,
            cmp: flip(cmp),
            color,
        },
        Shape::Delta {
            owner,
            result,
            cmp,
            limit,
        } => Shape::Delta {
            owner,
            result,
            cmp: flip(cmp),
            limit,
        },
        Shape::Same { .. } | Shape::Other => Shape::Other,
    }
}

/// `e` (or, with `neg`, `!e`) as an outline. A `!` is pushed down (De Morgan), so `!(a & b)`
/// reads as `!a | !b`; `a | b | c` is one group, not nested ones. A new request is never a saved
/// game, so `saved` is left out.
fn outline(e: &Expr, f: &Facts, neg: bool) -> Option<Outline> {
    let hit = eval(e, f) == Val::Bool(!neg);
    match &e.node {
        Node::Neg(a) if a.ty == Ty::Bool => outline(a, f, !neg),
        Node::Bin(op @ (Op::Or | Op::And), a, b) => {
            let any = (*op == Op::Or) != neg;
            let mut kids = Vec::new();
            for x in [a, b] {
                match outline(x, f, neg) {
                    Some(Outline::Group {
                        any: k, kids: ks, ..
                    }) if k == any => kids.extend(ks),
                    Some(o) => kids.push(o),
                    None => {}
                }
            }
            match kids.len() {
                0 => None,
                1 => kids.pop(),
                _ => Some(Outline::Group { any, hit, kids }),
            }
        }
        _ => {
            let shape = shape_of(e);
            let shape = if neg { negate(shape) } else { shape };
            if matches!(shape, Shape::Flag { var: "saved", .. }) {
                return None;
            }
            Some(Outline::Cond { shape, hit })
        }
    }
}

/// `formula` as an outline when it turns this request away; `None` when it does not, or when that
/// cannot be told before sending (an unknown value decides it, or the text does not parse).
pub fn declined_by(formula: &str, f: &Facts) -> Option<Outline> {
    let text = formula.trim();
    if text.is_empty() {
        return None;
    }
    let mut p = Parser { s: text, at: 0 };
    let e = p.e1().ok()?;
    if p.at < text.len() || e.ty != Ty::Bool {
        return None;
    }
    if eval(&e, f) != Val::Bool(true) {
        return None;
    }
    outline(&e, f, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIGLET: &str = "!saved & (size!=8 | anti | !synchro | !rand | mc!=? | discs<16 | discs>20 | mt1<200 | mm1!=0 | ml1==F | mt1 != ot1 | mt2 != ot2 |mt3 != ot3 | stored > 0 | md(+1.0) <= 0.0)";

    fn facts(gtype: &str, time: &str) -> Facts {
        let mut f = Facts::from_request(gtype, time).unwrap();
        f.rated = true;
        f.asker_rating = Some((2590.0, 60.0));
        f.owner_rating = Some((2380.6, 34.0));
        f
    }

    /// The conditions at the top of the outline that hold.
    fn hits(o: &Outline) -> Vec<Shape> {
        match o {
            Outline::Group { kids, .. } => kids
                .iter()
                .filter_map(|k| match k {
                    Outline::Cond { shape, hit: true } => Some(shape.clone()),
                    _ => None,
                })
                .collect(),
            Outline::Cond { shape, .. } => vec![shape.clone()],
        }
    }

    fn flag(var: &'static str, when: bool) -> Shape {
        Shape::Flag { var, when }
    }

    fn num(var: &'static str, cmp: Cmp, limit: f64) -> Shape {
        Shape::Number { var, cmp, limit }
    }

    #[test]
    fn piglets_formula_is_one_any_group_with_four_conditions_a_plain_three_minute_game_breaks() {
        let o = declined_by(PIGLET, &facts("8", "00:03:00")).unwrap();
        let Outline::Group {
            any: true, kids, ..
        } = &o
        else {
            panic!("{o:?}")
        };
        assert_eq!(kids.len(), 15);
        assert_eq!(
            hits(&o),
            [
                flag("synchro", false),
                flag("rand", false),
                num("discs", Cmp::Lt, 16.0),
                num("mt1", Cmp::Lt, 200.0)
            ]
        );
    }

    #[test]
    fn a_random_game_short_of_discs_and_time_still_fails_two_conditions() {
        let o = declined_by(PIGLET, &facts("s8r14", "00:03:00")).unwrap();
        assert_eq!(
            hits(&o),
            [num("discs", Cmp::Lt, 16.0), num("mt1", Cmp::Lt, 200.0)]
        );
    }

    #[test]
    fn what_piglet_asks_for_goes_through() {
        assert!(declined_by(PIGLET, &facts("s8r16", "00:05:00")).is_none());
    }

    #[test]
    fn a_far_weaker_asker_is_turned_away_by_the_rating_change() {
        let mut f = facts("s8r16", "00:05:00");
        f.asker_rating = Some((1500.0, 50.0));
        let o = declined_by(PIGLET, &f).unwrap();
        assert!(matches!(
            hits(&o)[..],
            [Shape::Delta {
                owner: true,
                cmp: Cmp::Le,
                ..
            }]
        ));
    }

    #[test]
    fn an_unknown_rating_leaves_the_decision_to_the_server() {
        let mut f = facts("s8r16", "00:05:00");
        f.asker_rating = None;
        assert!(declined_by(PIGLET, &f).is_none());
        assert!(declined_by("rnd(10) > 5", &f).is_none());
    }

    #[test]
    fn and_and_or_share_one_level_and_bind_to_the_right() {
        // `T & F | T` is `T & (F | T)` on the server, not `(T & F) | T`.
        let f = facts("8", "5");
        assert!(declined_by("T & F | T", &f).is_some());
        assert!(declined_by("F & T | T", &f).is_none());
    }

    #[test]
    fn a_clock_fills_what_it_leaves_out_from_othellos_default() {
        let c = Clock::parse("00:03:00").unwrap();
        assert_eq!(
            (c.ini, c.inc, c.ext, c.ini_moves, c.ini_loss),
            (180, 0, 120, 0, true)
        );
        let c = Clock::parse("5,N0/10/").unwrap();
        assert_eq!((c.ini, c.ini_loss, c.inc, c.ext), (300, false, 600, 120));
    }

    #[test]
    fn the_pool_is_size_plus_r_and_a() {
        assert_eq!(Facts::from_request("s8r16", "5").unwrap().pool(), "8r");
        assert_eq!(Facts::from_request("8", "5").unwrap().pool(), "8");
        assert_eq!(Facts::from_request("8b", "5").unwrap().asker_color, 'B');
    }

    #[test]
    fn an_and_inside_an_or_stays_a_group_of_its_own() {
        // Synchro needs 5 minutes, standard 10: a 3-minute synchro game breaks the first branch.
        let o = declined_by(
            "(synchro & mt1<300) | (!synchro & mt1<600)",
            &facts("s8r16", "00:03:00"),
        )
        .unwrap();
        let Outline::Group {
            any: true, kids, ..
        } = &o
        else {
            panic!("{o:?}")
        };
        assert_eq!(
            kids[0],
            Outline::Group {
                any: false,
                hit: true,
                kids: vec![
                    Outline::Cond {
                        shape: flag("synchro", true),
                        hit: true
                    },
                    Outline::Cond {
                        shape: num("mt1", Cmp::Lt, 300.0),
                        hit: true
                    },
                ]
            }
        );
        assert!(!kids[1].hit());
    }

    #[test]
    fn a_negated_group_is_read_condition_by_condition() {
        // !(rand & discs>=16) is "not random, or under 16 discs".
        let o = declined_by("!(rand & discs>=16)", &facts("8", "5")).unwrap();
        assert!(matches!(o, Outline::Group { any: true, .. }));
        assert_eq!(hits(&o), [flag("rand", false), num("discs", Cmp::Lt, 16.0)]);
    }

    #[test]
    fn saved_is_left_out() {
        let o = declined_by("!saved & synchro", &facts("s8r16", "5")).unwrap();
        assert_eq!(
            o,
            Outline::Cond {
                shape: flag("synchro", true),
                hit: true
            }
        );
    }
}
