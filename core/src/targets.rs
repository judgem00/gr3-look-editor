//! Targets (docs/plan-gui-redesign.md): the image controls that get a look
//! of the user's, each built from a source look. A target is independent of
//! every other look: where it would share its ColorMatrix set with its source
//! (or, for Standard, with HDR in stock firmware) it gets a set of its own
//! (looks::private_set_at), checked in Unicorn by tools/verify-targets.py.
//!
//! - A target that is its own source keeps its stock structure: Bleach Bypass
//!   keeps its three variants, Retro its toning.
//! - Bleach Bypass built from another source is one look: tables 18, 22 and
//!   23 share its private set.
//! - HDR takes another source or a matrix of its own only with its effects off.
//! - A target can have grain (grain.rs), whether its look offers it or not.
//! - A target can have a post matrix P after the tone curve (postmatrix.rs).

use crate::looks::{self, Toning, BLEACH_TABLES, SET_SIZE, TONING_TABLE};
use serde_json::{json, Value};

/// image controls that can be targets, in the camera menu's order
pub const TARGETS: [&str; 8] = [
    "Standard", "Vivid", "PositiveFilm", "BleechBypass_GR", "Retro", "HdrCuisine", "CrossProcess", "NegativeFilm",
];
pub const CROSS: &str = "CrossProcess";
pub const HDR: &str = "HdrCuisine";
const BLEACH: &str = "BleechBypass_GR";

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub name: String,
    pub source: String,
    /// HDR only: its tone mapping and strength off (curvesel::hdr_effects_off)
    pub effects_off: bool,
    /// grain 1..3 whatever the camera's setting; 0 leaves it to the camera
    pub grain: u8,
    /// 3x3 after the tone curve, rows summing to 1 (postmatrix.rs)
    pub post: Option<[f64; 9]>,
}

/// looks a target can start from: every colour look and table variant except
/// HDR's monochrome variant (table 24)
pub fn sources() -> impl Iterator<Item = &'static str> {
    looks::LOOKS.iter()
        .filter(|l| !matches!(l.0, 13..=16) && l.1 != 24)
        .map(|l| l.2)
}

fn table(name: &str) -> u32 {
    looks::table_of(name).unwrap()
}

impl Target {
    pub fn new(name: &str, source: Option<&str>, effects_off: bool) -> Result<Target, String> {
        if !TARGETS.contains(&name) {
            return Err(format!("{name:?} cannot be a target"));
        }
        let source = source.unwrap_or(name);
        if !sources().any(|s| s == source) {
            return Err(format!("{source:?} cannot be a source"));
        }
        if effects_off && name != HDR {
            return Err(format!("effects can only be switched off for {HDR}, not {name:?}"));
        }
        if name == HDR && source != HDR && !effects_off {
            return Err(format!("{HDR} takes another source only with its effects off"));
        }
        Ok(Target { name: name.into(), source: source.into(), effects_off, grain: 0, post: None })
    }

    pub fn with_grain(mut self, grain: u8) -> Result<Target, String> {
        if grain > crate::grain::MAX {
            return Err(format!("grain {grain} for {}: 0..{}", self.name, crate::grain::MAX));
        }
        self.grain = grain;
        Ok(self)
    }

    /// an identity P is the same as none
    pub fn with_post(mut self, post: Option<[f64; 9]>) -> Target {
        const ID: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        self.post = post.filter(|p| *p != ID);
        self
    }

    /// does its colour data come from another look?
    pub fn foreign(&self) -> bool {
        self.source != self.name
    }

    /// the look tables it covers
    pub fn tables(&self) -> Vec<u32> {
        if self.name == BLEACH && self.foreign() { BLEACH_TABLES.to_vec() } else { vec![table(&self.name)] }
    }

    /// the tables its post matrix goes to: Bleach Bypass's three variants always
    pub fn post_tables(&self) -> Vec<u32> {
        if self.name == BLEACH { BLEACH_TABLES.to_vec() } else { self.tables() }
    }

    /// does it get a ColorMatrix set of its own, and is that set at a fixed place?
    fn private(&self) -> Option<Option<u32>> {
        match self.name.as_str() {
            CROSS => Some(looks::fixed_set(table(CROSS))),
            HDR => self.effects_off.then(|| looks::fixed_set(table(HDR))),
            "Standard" => Some(None),
            _ => self.foreign().then_some(None),
        }
    }

    /// may the export write a matrix under this look name for it?
    pub fn owns_matrix(&self, look: &str) -> bool {
        if self.name == HDR && !self.effects_off {
            return false;
        }
        look == self.name || (self.name == BLEACH && looks::table_of(look).is_some_and(|t| BLEACH_TABLES.contains(&t)))
    }

    pub fn to_json(&self) -> Value {
        json!({ "name": self.name, "source": self.source, "effects_off": self.effects_off, "grain": self.grain,
                "post": self.post })
    }
}

/// a small item for space::plan_with
#[derive(Debug, Clone, Copy, PartialEq)]
enum Item {
    Set(usize),
    Toning(usize),
    /// its own hue table with P
    Post(usize),
    /// the grain stubs and their table, once for all targets
    Grain,
}

fn toning(stock: &[u8], t: &Target) -> Toning {
    Toning::of(stock, t.tables()[0], table(&t.source))
}

/// the small items `targets` need, in a fixed order, with their sizes
fn items(stock: &[u8], targets: &[Target]) -> Vec<(Item, u32)> {
    let mut out = vec![];
    for (k, t) in targets.iter().enumerate() {
        if t.private() == Some(None) {
            out.push((Item::Set(k), SET_SIZE));
        }
        if toning(stock, t).needs_tables() {
            out.push((Item::Toning(k), 2 * TONING_TABLE));
        }
        if t.post.is_some() {
            out.push((Item::Post(k), crate::postmatrix::SIZE));
        }
    }
    if targets.iter().any(|t| t.grain > 0) {
        out.push((Item::Grain, crate::grain::SIZE));
    }
    out
}

pub fn item_sizes(stock: &[u8], targets: &[Target]) -> Vec<u32> {
    items(stock, targets).into_iter().map(|i| i.1).collect()
}

/// Apply `targets` to `img` (a copy of `stock`): sources first, read from
/// stock so that targets never chain, then the private sets. `places` are
/// the addresses space::plan_with gave for item_sizes, in that order.
pub fn apply(img: &mut [u8], stock: &[u8], targets: &[Target], places: &[u32]) -> Result<(), String> {
    let its = items(stock, targets);
    if its.len() != places.len() {
        return Err("targets and their places do not match".into());
    }
    let place = |want: Item| its.iter().zip(places).find(|(i, _)| i.0 == want).map(|(_, &a)| a);
    for t in targets.iter().filter(|t| t.foreign()) {
        for tb in t.tables() {
            looks::inherit_from(img, stock, tb, table(&t.source));
        }
    }
    for (k, t) in targets.iter().enumerate() {
        let Some(fixed) = t.private() else { continue };
        let set = match fixed {
            Some(a) => a,
            None => place(Item::Set(k)).ok_or("no place for a colour set")?,
        };
        looks::private_set_at(img, &t.tables(), set, toning(stock, t), place(Item::Toning(k)))?;
    }
    // after inherit_from: P works on the hue matrices the target uses now
    for (k, t) in targets.iter().enumerate() {
        if let Some(p) = &t.post {
            let at = place(Item::Post(k)).ok_or("no place for a post matrix")?;
            crate::postmatrix::install(img, &t.post_tables(), at, p).map_err(|e| format!("{}: {e}", t.name))?;
        }
    }
    let grain: Vec<(u32, u8)> = targets.iter().filter(|t| t.grain > 0).map(|t| (looks::index_of(&t.name), t.grain)).collect();
    if !grain.is_empty() {
        crate::grain::install(img, place(Item::Grain).ok_or("no place for the grain stubs")?, &grain)?;
    }
    Ok(())
}

/// targets in TARGETS order, each name once
pub fn sorted(mut targets: Vec<Target>) -> Result<Vec<Target>, String> {
    targets.sort_by_key(|t| TARGETS.iter().position(|n| *n == t.name));
    if targets.windows(2).any(|w| w[0].name == w[1].name) {
        return Err("a target is given twice".into());
    }
    Ok(targets)
}
