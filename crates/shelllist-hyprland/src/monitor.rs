//! Typed encoding for Hyprland's Lua monitor rules, without executing effects.
//! Callers own eligibility, topology/mode validation, persistence and readback.
use crate::Lua;
use anyhow::{Context, Result, ensure};

/// A compositor-selected mode or an explicit `WIDTHxHEIGHT@RATE[Hz]` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode<'a> {
    Preferred,
    Exact(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Auto,
    Exact { x: i32, y: i32 },
}

/// Explicit enablement, not an arbitrary Lua table or a daemon's saved layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule<'a> {
    Disable,
    Enable {
        mode: Mode<'a>,
        position: Position,
        scale: f64,
        /// None preserves the existing rule's transform; Some(0) resets it.
        transform: Option<u8>,
        /// Empty explicitly clears mirroring; otherwise an exact output name.
        mirror: &'a str,
    },
}

/// Optional, explicitly managed monitor attributes. None leaves a rule alone.
/// VRR is restricted to observable off/on; fullscreen policies cannot be
/// distinguished by Hyprland's boolean monitor readback.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Advanced<'a> {
    pub vrr: Option<bool>,
    pub bit_depth: Option<u8>,
    pub color: Option<&'a str>,
    pub sdr_brightness: Option<f64>,
    pub sdr_saturation: Option<f64>,
}
impl Advanced<'_> {
    pub fn validate(self) -> Result<()> {
        ensure!(
            self.bit_depth.is_none_or(|v| matches!(v, 8 | 10)),
            "Invalid monitor bit depth"
        );
        ensure!(
            self.color.is_none_or(|v| [
                "srgb", "wide", "edid", "dcip3", "dp3", "adobe", "hdr", "hdredid"
            ]
            .contains(&v)),
            "Invalid monitor color preset"
        );
        ensure!(
            self.sdr_brightness
                .is_none_or(|v| v.is_finite() && (0.1..=3.0).contains(&v)),
            "Invalid SDR brightness"
        );
        ensure!(
            self.sdr_saturation
                .is_none_or(|v| v.is_finite() && (0.0..=2.0).contains(&v)),
            "Invalid SDR saturation"
        );
        Ok(())
    }
    fn fields(self) -> Result<String> {
        self.validate()?;
        let mut fields = String::new();
        if let Some(v) = self.vrr {
            fields.push_str(&format!(", vrr = {}", u8::from(v)));
        }
        if let Some(v) = self.bit_depth {
            fields.push_str(&format!(", bitdepth = {v}"));
        }
        if let Some(v) = self.color {
            fields.push_str(&format!(", cm = \"{v}\""));
        }
        if let Some(v) = self.sdr_brightness {
            fields.push_str(&format!(", sdrbrightness = {v}"));
        }
        if let Some(v) = self.sdr_saturation {
            fields.push_str(&format!(", sdrsaturation = {v}"));
        }
        Ok(fields)
    }
}

/// Encode one `eval hl.monitor(...)` request for [`crate::Client::request_exact`].
/// Use exact targeting so a stale session cannot redirect mutations elsewhere.
///
/// Accepts exact ASCII connector identifiers, not selectors, expressions or Lua.
/// Validation here protects command syntax, finite positive scales/modes and the
/// compositor's transform enum. Domain limits, advertised modes, last-screen
/// protection and confirmation against observed state remain caller obligations.
/// This function performs no IPC, retries, persistence or implicit readback.
pub fn command(output: &str, rule: Rule<'_>) -> Result<Lua> {
    command_with_advanced(output, rule, Advanced::default())
}

pub fn command_with_advanced(output: &str, rule: Rule<'_>, advanced: Advanced<'_>) -> Result<Lua> {
    ensure!(identifier(output), "Invalid Hyprland output identifier");
    let advanced = advanced.fields()?;
    let Rule::Enable {
        mode,
        position,
        scale,
        transform,
        mirror,
    } = rule
    else {
        return Ok(Lua(format!(
            "hl.monitor({{ output = \"{output}\", disabled = true }})"
        )));
    };
    let mode = mode.token()?;
    ensure!(
        scale.is_finite() && scale > 0.0,
        "Invalid Hyprland monitor scale"
    );
    ensure!(
        transform.is_none_or(|value| value <= 7),
        "Invalid Hyprland monitor transform"
    );
    ensure!(
        mirror.is_empty() || identifier(mirror),
        "Invalid Hyprland mirror identifier"
    );
    let position = match position {
        Position::Auto => "auto".into(),
        Position::Exact { x, y } => format!("{x}x{y}"),
    };
    let transform = transform.map_or_else(String::new, |value| format!(", transform = {value}"));
    // hl.monitor merges with the previous rule. Supplying geometry alone does
    // not undo disabled=true, and omitting mirror would retain an old source.
    Ok(Lua(format!(
        "hl.monitor({{ output = \"{output}\", disabled = false, mode = \"{mode}\", position = \"{position}\", scale = {scale}{transform}, mirror = \"{mirror}\"{advanced} }})"
    )))
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
}

impl<'a> Mode<'a> {
    fn token(self) -> Result<&'a str> {
        let Self::Exact(value) = self else {
            return Ok("preferred");
        };
        let value = value.strip_suffix("Hz").unwrap_or(value);
        ensure!(
            value
                .bytes()
                .all(|c| c.is_ascii_digit() || b"x@.".contains(&c)),
            "Invalid Hyprland monitor mode"
        );
        let (size, rate) = value
            .split_once('@')
            .context("Monitor mode needs a refresh rate")?;
        let (width, height) = size
            .split_once('x')
            .context("Monitor mode needs dimensions")?;
        let width: u32 = width.parse().context("Invalid monitor width")?;
        let height: u32 = height.parse().context("Invalid monitor height")?;
        let rate: f64 = rate.parse().context("Invalid monitor refresh rate")?;
        ensure!(
            width > 0 && height > 0 && rate.is_finite() && rate > 0.0,
            "Invalid Hyprland monitor mode"
        );
        Ok(value)
    }
}

#[cfg(test)]
mod tests;
