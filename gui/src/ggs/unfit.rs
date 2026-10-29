//! The opponent's decline formula checked against a request before it is sent, and put in words.

use super::*;

/// One line of the opponent's formula as the request sheet lays it out.
#[derive(Clone, Serialize)]
pub struct UnfitRow {
    pub depth: u8,
    /// `Some(any)` for a group ("any of" / "all of"), `None` for a condition.
    pub group: Option<bool>,
    pub text: String,
    pub hit: bool,
}

/// Why the opponent turns the request away: the top of its formula (`any`, `all`, or `one` for a
/// single condition) and the lines below it.
#[derive(Clone, Serialize)]
pub struct UnfitView {
    pub top: &'static str,
    pub rows: Vec<UnfitRow>,
}

/// `opp`'s decline formula laid out for the request sheet; `None` when the request fits.
pub fn unfit_view(
    s: &Snapshot,
    gtype: &str,
    time: &str,
    opp: &str,
    rated: bool,
) -> Option<UnfitView> {
    let o = unfit_ask(s, gtype, time, opp, rated)?;
    let mut rows = Vec::new();
    let top = match &o {
        formula::Outline::Group { any, kids, .. } => {
            for k in hits_first(kids) {
                unfit_rows(k, opp, 0, &mut rows);
            }
            if *any {
                "any"
            } else {
                "all"
            }
        }
        formula::Outline::Cond { .. } => {
            unfit_rows(&o, opp, 0, &mut rows);
            "one"
        }
    };
    Some(UnfitView { top, rows })
}

fn unfit_rows(o: &formula::Outline, opp: &str, depth: u8, out: &mut Vec<UnfitRow>) {
    match o {
        formula::Outline::Group { any, hit, kids } => {
            out.push(UnfitRow {
                depth,
                group: Some(*any),
                text: String::new(),
                hit: *hit,
            });
            for k in hits_first(kids) {
                unfit_rows(k, opp, depth + 1, out);
            }
        }
        formula::Outline::Cond { shape, hit } => out.push(UnfitRow {
            depth,
            group: None,
            text: unfit_words(shape, opp),
            hit: *hit,
        }),
    }
}

/// The parts of a group with the ones that hold first; their order inside `|` or `&` means nothing.
fn hits_first(kids: &[formula::Outline]) -> impl Iterator<Item = &formula::Outline> {
    kids.iter()
        .filter(|k| k.hit())
        .chain(kids.iter().filter(|k| !k.hit()))
}

/// The conditions that hold, in words, for a notice: a group that holds reads as "all of: a, b".
pub(super) fn hit_words(o: &formula::Outline, opp: &str) -> String {
    use crate::i18n::t;
    match o {
        formula::Outline::Cond { shape, .. } => unfit_words(shape, opp),
        formula::Outline::Group { any, kids, .. } => {
            let parts: Vec<String> = kids
                .iter()
                .filter(|k| k.hit())
                .map(|k| match k {
                    formula::Outline::Group { any: inner, .. } => format!(
                        "{}: {}",
                        t(if *inner {
                            "backend.unfit.group_any"
                        } else {
                            "backend.unfit.group_all"
                        }),
                        hit_words(k, opp)
                    ),
                    _ => hit_words(k, opp),
                })
                .collect();
            let sep = if *any {
                " / ".to_string()
            } else {
                t("backend.unfit.join")
            };
            parts.join(&sep)
        }
    }
}

/// `opp`'s decline formula as an outline when it turns this request away, judged before sending
/// as the server would; `None` when it fits, or when that cannot be told (finger not read, a value
/// unknown).
pub(super) fn unfit_ask(
    s: &Snapshot,
    gtype: &str,
    time: &str,
    opp: &str,
    rated: bool,
) -> Option<formula::Outline> {
    if opp.is_empty() || opp == s.login {
        return None;
    }
    let finger = s.fingers.get(opp)?;
    let decline = &finger.fields.iter().find(|(k, _)| k == "decline")?.1;
    let mut f = formula::Facts::from_request(gtype, time)?;
    f.rated = rated && !no_rated();
    f.stored = s.stored.iter().filter(|x| x.opp == opp).count() as i64;
    let pool = f.pool();
    // Someone online with no rating in the pool is rated from the server's start, 1720 @ 350.
    let rating = |name: &str| {
        let u = s.users.iter().find(|u| u.name == name)?;
        let r = match pool.as_str() {
            "8" => u.rating.zip(u.dev),
            "8r" => u.rating_r.zip(u.dev_r),
            _ => return None,
        };
        Some(r.map_or((1720.0, 350.0), |(r, d)| (f64::from(r), f64::from(d))))
    };
    f.asker_rating = rating(&s.login);
    f.owner_rating = rating(opp);
    formula::declined_by(decline, &f)
}

/// One condition of the opponent's decline formula in words, as a state of the request that the
/// condition looks for ("not synchro", "the main time under 3 min 20 s").
fn unfit_words(shape: &formula::Shape, owner: &str) -> String {
    use crate::i18n::{t, tf};
    use formula::{Cmp, Shape};
    let key = |k: &str| format!("backend.unfit.{k}");
    let other = || t(&key("other"));
    // The clock variables come in pairs (`mt1` / `ot1`): the owner's and the asker's are worded alike.
    let base = |var: &str| -> String {
        if var.len() == 3 && (var.starts_with('m') || var.starts_with('o')) {
            var[1..].to_string()
        } else {
            var.to_string()
        }
    };
    // A key the language does not have comes back as itself; such a condition is said generically.
    let label = |var: &str| {
        let k = key(&format!("var.{}", base(var)));
        let l = tf(&k, &[("name", owner)]);
        (l != k).then_some(l)
    };
    let secs = |v: f64| {
        let v = v.round() as i64;
        let (m, s) = ((v / 60).to_string(), (v % 60).to_string());
        let k = match (v / 60, v % 60) {
            (0, _) => "time_s",
            (_, 0) => "time_m",
            _ => "time_ms",
        };
        tf(&key(k), &[("m", &m), ("s", &s)])
    };
    let value = |var: &str, v: f64| match base(var).as_str() {
        "t1" | "t2" | "t3" => secs(v),
        "mr" | "or" => format!("{v:.1}"),
        _ => format!("{}", v.round() as i64),
    };
    match shape {
        Shape::Flag { var, when } => {
            let k = key(&format!(
                "flag.{}_{}",
                base(var),
                if *when { "yes" } else { "no" }
            ));
            let w = t(&k);
            if w == k {
                other()
            } else {
                w
            }
        }
        Shape::Number {
            var: "stored",
            cmp: Cmp::Gt,
            limit,
        } if *limit == 0.0 => t(&key("stored")),
        Shape::Number { var, cmp, limit } => {
            let Some(label) = label(var) else {
                return other();
            };
            let k = match cmp {
                Cmp::Lt => "number_lt",
                Cmp::Le => "number_le",
                Cmp::Gt => "number_gt",
                Cmp::Ge => "number_ge",
                Cmp::Eq => "number_eq",
                Cmp::Ne => "number_ne",
            };
            // A number is spaced from the words after it ("16 未満"), a unit is not ("3 分未満").
            let v = value(var, *limit);
            let sp = if v.ends_with(|c: char| c.is_ascii_digit()) {
                " "
            } else {
                ""
            };
            tf(&key(k), &[("label", &label), ("v", &v), ("sp", sp)])
        }
        Shape::Same { var } => match label(var) {
            Some(label) => tf(&key("same"), &[("label", &label)]),
            None => other(),
        },
        Shape::Color { var, cmp, color } => {
            // The asker's own colour has its own words: "you" is not a name.
            let you = if *var == "mc" { "" } else { "_you" };
            match (color, cmp) {
                ('?', Cmp::Ne) => t(&key("color_named")),
                ('?', Cmp::Eq) => t(&key("color_unnamed")),
                ('B' | 'W', Cmp::Eq | Cmp::Ne) => {
                    let side = t(&key(if *color == 'B' { "black" } else { "white" }));
                    let k = if *cmp == Cmp::Eq {
                        "color_is"
                    } else {
                        "color_is_not"
                    };
                    tf(
                        &key(&format!("{k}{you}")),
                        &[("who", owner), ("side", &side)],
                    )
                }
                _ => other(),
            }
        }
        Shape::Delta {
            owner: true,
            result,
            cmp: Cmp::Le | Cmp::Lt,
            limit,
        } if *result > 0.0 && *limit <= 0.0 => t(&key("no_gain")),
        Shape::Delta { .. } => t(&key("rating")),
        Shape::Other => other(),
    }
}
