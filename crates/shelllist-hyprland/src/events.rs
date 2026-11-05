use crate::{WindowAddress, workspace::WorkspaceRef};

/// Only identity read by consumers is typed; the remaining notifications carry
/// no presentation data. Unknown names remain forward compatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositorEvent {
    WorkspaceChanged,
    WorkspaceCreated,
    WorkspaceDestroyed,
    WorkspaceMoved,
    WorkspaceRenamed,
    SpecialWorkspaceChanged,
    FocusedMonitorChanged,
    MonitorAdded,
    MonitorRemoved,
    WindowOpened,
    WindowClosed,
    WindowMoved {
        address: WindowAddress,
        workspace: WorkspaceRef,
    },
    ActiveWindowChanged,
    WindowTitleChanged,
    WindowUrgent,
    FullscreenChanged,
    FloatingChanged,
    WindowPinned,
    GroupToggled,
    WindowEnteredGroup,
    WindowLeftGroup,
    LayerOpened,
    LayerClosed,
    ConfigReloaded,
    WorkspaceIdChanged,
    Malformed {
        name: String,
        data: String,
    },
    Unknown {
        name: String,
        data: String,
    },
}

impl CompositorEvent {
    pub fn parse(line: &str) -> Self {
        let (name, data) = line.split_once(">>").unwrap_or((line, ""));
        // Keep validation fallible until one common malformed-payload boundary.
        let parsed = (|| {
            Some(match name {
                "workspace" | "workspacev2" => Self::WorkspaceChanged,
                "createworkspace" | "createworkspacev2" => Self::WorkspaceCreated,
                "destroyworkspace" | "destroyworkspacev2" => Self::WorkspaceDestroyed,
                "moveworkspace" | "moveworkspacev2" => Self::WorkspaceMoved,
                "renameworkspace" => Self::WorkspaceRenamed,
                "activespecial" | "activespecialv2" => Self::SpecialWorkspaceChanged,
                "focusedmon" | "focusedmonv2" => Self::FocusedMonitorChanged,
                "monitoradded" | "monitoraddedv2" => Self::MonitorAdded,
                "monitorremoved" | "monitorremovedv2" => Self::MonitorRemoved,
                "openwindow" => {
                    let mut fields = data.splitn(4, ',');
                    return fields
                        .next()
                        .and_then(WindowAddress::from_digits)
                        .and(fields.nth(2))
                        .map(|_| Self::WindowOpened);
                }
                "closewindow" => {
                    return WindowAddress::from_digits(data).map(|_| Self::WindowClosed);
                }
                "movewindow" | "movewindowv2" => {
                    let mut fields = data.splitn(if name.ends_with("v2") { 3 } else { 2 }, ',');
                    let address = fields.next().and_then(WindowAddress::from_digits)?;
                    let identity = fields.next().filter(|v| !v.is_empty())?;
                    let display = if name.ends_with("v2") {
                        fields.next()?
                    } else {
                        identity
                    };
                    let special = identity.starts_with("special:");
                    let number = identity.parse::<i64>().ok();
                    let id = number.filter(|id| *id > 0);
                    if identity == "special:"
                        || number.is_some_and(|id| id <= 0)
                        || (identity.bytes().all(|byte| byte.is_ascii_digit()) && id.is_none())
                    {
                        return None;
                    }
                    Self::WindowMoved {
                        address,
                        workspace: WorkspaceRef {
                            id,
                            address: Some(identity.into()),
                            name: display.into(),
                            special,
                        },
                    }
                }
                "activewindow" => Self::ActiveWindowChanged,
                "activewindowv2" => {
                    if !data.is_empty() {
                        WindowAddress::from_digits(data)?;
                    }
                    Self::ActiveWindowChanged
                }
                "windowtitle" => {
                    return WindowAddress::from_digits(data).map(|_| Self::WindowTitleChanged);
                }
                "windowtitlev2" => {
                    return data
                        .split_once(',')
                        .and_then(|(address, _)| WindowAddress::from_digits(address))
                        .map(|_| Self::WindowTitleChanged);
                }
                "urgent" => Self::WindowUrgent,
                "fullscreen" => Self::FullscreenChanged,
                "changefloatingmode" => Self::FloatingChanged,
                "pin" => Self::WindowPinned,
                "togglegroup" => Self::GroupToggled,
                "moveintogroup" => Self::WindowEnteredGroup,
                "moveoutofgroup" => Self::WindowLeftGroup,
                "openlayer" => Self::LayerOpened,
                "closelayer" => Self::LayerClosed,
                "configreloaded" => Self::ConfigReloaded,
                "changeworkspaceid" => {
                    return data
                        .split_once(',')
                        .filter(|(old, new)| {
                            [old, new]
                                .iter()
                                .all(|v| v.parse::<i64>().is_ok_and(|id| id > 0))
                        })
                        .map(|_| Self::WorkspaceIdChanged);
                }
                _ => Self::Unknown {
                    name: name.into(),
                    data: data.into(),
                },
            })
        })();
        match parsed {
            Some(event) if line.contains(">>") || matches!(event, Self::Unknown { .. }) => event,
            _ => Self::Malformed {
                name: name.into(),
                data: data.into(),
            },
        }
    }

    pub fn invalidates_geometry(&self) -> bool {
        !matches!(
            self,
            Self::ActiveWindowChanged
                | Self::WindowTitleChanged
                | Self::WindowUrgent
                | Self::Unknown { .. }
        )
    }
    pub fn invalidates_preferences(&self) -> bool {
        matches!(self, Self::ConfigReloaded | Self::Malformed { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::CompositorEvent;
    use crate::{WindowAddress, workspace::WorkspaceRef};
    use CompositorEvent::{
        ActiveWindowChanged, Malformed, Unknown, WindowMoved, WindowOpened, WindowTitleChanged,
        WorkspaceIdChanged,
    };

    #[test]
    fn captures_and_exact_names_cover_the_old_geometry_prefixes() {
        for name in "workspace workspacev2 createworkspace createworkspacev2 \
            destroyworkspace destroyworkspacev2 moveworkspace moveworkspacev2 \
            renameworkspace activespecial activespecialv2 focusedmon focusedmonv2 \
            monitoradded monitoraddedv2 monitorremoved monitorremovedv2 openwindow \
            closewindow movewindow movewindowv2 changefloatingmode fullscreen \
            togglegroup moveintogroup moveoutofgroup pin openlayer closelayer \
            configreloaded changeworkspaceid"
            .split_whitespace()
        {
            let data = match name {
                "openwindow" => "123,6,app,title,with,commas",
                "closewindow" => "123",
                "movewindow" => "123,6",
                "movewindowv2" => "123,6,display,name",
                "changeworkspaceid" => "6,7",
                _ => "fixture",
            };
            let event = CompositorEvent::parse(&format!("{name}>>{data}"));
            assert!(
                !matches!(event, Unknown { .. } | Malformed { .. }),
                "{name}"
            );
            assert!(event.invalidates_geometry(), "{name}");
            assert!(matches!(
                CompositorEvent::parse(&format!("{name}-future>>{data}")),
                Unknown { .. }
            ));
        }
        for line in include_str!("../tests/fixtures/0.57.1/events.txt").lines() {
            let event = CompositorEvent::parse(line);
            assert!(!matches!(event, Malformed { .. }), "{line}");
            if !line.starts_with("activelayout>>") {
                assert!(!matches!(event, Unknown { .. }), "{line}");
            }
        }
    }

    #[test]
    fn moves_preserve_numbered_named_and_special_identity() -> anyhow::Result<()> {
        for (identity, id, special, selector) in [
            ("7", Some(7), false, "7"),
            ("+7", Some(7), false, "7"),
            ("project", None, false, "name:project"),
            ("special:probe", None, true, "special:probe"),
        ] {
            // v2 appends displayName; a numeric display never replaces identity.
            for (name, display) in [("movewindow", identity), ("movewindowv2", "6,with,commas")] {
                let data = if name == "movewindowv2" {
                    format!("AbC,{identity},{display}")
                } else {
                    format!("AbC,{identity}")
                };
                let event = CompositorEvent::parse(&format!("{name}>>{data}"));
                let workspace = WorkspaceRef {
                    id,
                    address: Some(identity.into()),
                    name: display.into(),
                    special,
                };
                assert_eq!(workspace.selector()?, selector);
                assert_eq!(
                    event,
                    WindowMoved {
                        address: WindowAddress::parse("0xabc")?,
                        workspace,
                    }
                );
            }
        }
        Ok(())
    }

    #[test]
    fn known_payloads_validate_fields_and_preserve_malformed_data() {
        for line in "movewindowv2>>123,1 movewindow>>123, movewindowv2>>123,,name \
            movewindowv2>>123,special:,name movewindowv2>>123,0,zero \
            movewindowv2>>123,-2,negative movewindowv2>>123,18446744073709551615,overflow \
            openwindow>> openwindow>>123,1,app windowtitlev2>>123 activewindowv2>>bad! \
            changeworkspaceid>>a,2 changeworkspaceid>>1 changeworkspaceid>>1,0 \
            changeworkspaceid>>-1,2 changeworkspaceid>>1,2,3 workspace activewindowv2"
            .split_whitespace()
            .map(str::to_owned)
            .chain(
                ["", "0", "000", "bad!", "0x123", "10000000000000000", "é"]
                    .into_iter()
                    .flat_map(|address| {
                        [
                            format!("closewindow>>{address}"),
                            format!("windowtitle>>{address}"),
                            format!("windowtitlev2>>{address},title"),
                            format!("movewindow>>{address},1"),
                            format!("openwindow>>{address},1,app,title"),
                        ]
                    }),
            )
        {
            let (name, data) = line.split_once(">>").unwrap_or((&line, ""));
            let event = CompositorEvent::parse(&line);
            assert_eq!(
                event,
                Malformed {
                    name: name.into(),
                    data: data.into()
                }
            );
            assert!(event.invalidates_geometry());
            assert!(event.invalidates_preferences());
        }
        for (line, expected) in [
            ("windowtitlev2>>123,title,with,commas", WindowTitleChanged),
            ("windowtitlev2>>123,", WindowTitleChanged),
            ("activewindowv2>>", ActiveWindowChanged),
            ("activewindowv2>>123", ActiveWindowChanged),
            ("openwindow>>123,,,", WindowOpened),
            ("changeworkspaceid>>1,2", WorkspaceIdChanged),
        ] {
            assert_eq!(CompositorEvent::parse(line), expected);
        }
        for (line, name, data) in [
            ("future>>a,b,c", "future", "a,b,c"),
            ("future", "future", ""),
        ] {
            let event = CompositorEvent::parse(line);
            assert_eq!(
                event,
                Unknown {
                    name: name.into(),
                    data: data.into()
                }
            );
            assert!(!event.invalidates_geometry());
            assert!(!event.invalidates_preferences());
        }
    }
}
