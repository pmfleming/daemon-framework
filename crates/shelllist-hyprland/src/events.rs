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
        let malformed = || Self::Malformed {
            name: name.into(),
            data: data.into(),
        };
        let parsed = match name {
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
                let fields: Vec<_> = data.splitn(4, ',').collect();
                if fields.len() == 4 && event_address(fields[0]).is_some() {
                    Self::WindowOpened
                } else {
                    malformed()
                }
            }
            "closewindow" => {
                if event_address(data).is_some() {
                    Self::WindowClosed
                } else {
                    malformed()
                }
            }
            "movewindow" | "movewindowv2" => {
                let mut fields = data.splitn(if name.ends_with("v2") { 3 } else { 2 }, ',');
                let Some(address) = fields.next().and_then(event_address) else {
                    return malformed();
                };
                let Some(identity) = fields.next().filter(|v| !v.is_empty()) else {
                    return malformed();
                };
                let display = if name.ends_with("v2") {
                    let Some(display) = fields.next() else {
                        return malformed();
                    };
                    display
                } else {
                    identity
                };
                let special = identity.starts_with("special:");
                let id = identity.parse::<i64>().ok().filter(|id| *id > 0);
                if identity == "special:"
                    || identity.parse::<i64>().is_ok_and(|id| id <= 0)
                    || (identity.bytes().all(|byte| byte.is_ascii_digit()) && id.is_none())
                {
                    return malformed();
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
                if data.is_empty() || event_address(data).is_some() {
                    Self::ActiveWindowChanged
                } else {
                    malformed()
                }
            }
            "windowtitle" => {
                if event_address(data).is_some() {
                    Self::WindowTitleChanged
                } else {
                    malformed()
                }
            }
            "windowtitlev2" => {
                if data
                    .split_once(',')
                    .is_some_and(|(address, _)| event_address(address).is_some())
                {
                    Self::WindowTitleChanged
                } else {
                    malformed()
                }
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
                if data.split_once(',').is_some_and(|(old, new)| {
                    [old, new]
                        .iter()
                        .all(|v| v.parse::<i64>().is_ok_and(|id| id > 0))
                }) {
                    Self::WorkspaceIdChanged
                } else {
                    malformed()
                }
            }
            _ => Self::Unknown {
                name: name.into(),
                data: data.into(),
            },
        };
        if !line.contains(">>") && !matches!(parsed, Self::Unknown { .. }) {
            malformed()
        } else {
            parsed
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

fn event_address(value: &str) -> Option<WindowAddress> {
    // The protocol carries bare hexadecimal pointers, never arbitrary selectors.
    WindowAddress::parse(&format!("0x{value}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_and_exact_names_cover_the_old_geometry_prefixes() {
        let names = [
            "workspace",
            "workspacev2",
            "createworkspace",
            "createworkspacev2",
            "destroyworkspace",
            "destroyworkspacev2",
            "moveworkspace",
            "moveworkspacev2",
            "renameworkspace",
            "activespecial",
            "activespecialv2",
            "focusedmon",
            "focusedmonv2",
            "monitoradded",
            "monitoraddedv2",
            "monitorremoved",
            "monitorremovedv2",
            "openwindow",
            "closewindow",
            "movewindow",
            "movewindowv2",
            "changefloatingmode",
            "fullscreen",
            "togglegroup",
            "moveintogroup",
            "moveoutofgroup",
            "pin",
            "openlayer",
            "closelayer",
            "configreloaded",
            "changeworkspaceid",
        ];
        for name in names {
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
                !matches!(
                    event,
                    CompositorEvent::Unknown { .. } | CompositorEvent::Malformed { .. }
                ),
                "{name}"
            );
            assert!(event.invalidates_geometry(), "{name}");
            assert!(matches!(
                CompositorEvent::parse(&format!("{name}-future>>{data}")),
                CompositorEvent::Unknown { .. }
            ));
        }
        for line in include_str!("../tests/fixtures/0.57.1/events.txt").lines() {
            let event = CompositorEvent::parse(line);
            assert!(
                !matches!(event, CompositorEvent::Malformed { .. }),
                "{line}"
            );
            if !line.starts_with("activelayout>>") {
                assert!(!matches!(event, CompositorEvent::Unknown { .. }), "{line}");
            }
        }
    }

    #[test]
    fn moves_preserve_special_identity_and_display_commas() {
        let CompositorEvent::WindowMoved { address, workspace } =
            CompositorEvent::parse("movewindowv2>>AbC,special:probe,display,with,commas")
        else {
            panic!("expected move");
        };
        assert_eq!(address.as_str(), "0xabc");
        assert_eq!(workspace.id, None);
        assert_eq!(workspace.address.as_deref(), Some("special:probe"));
        assert_eq!(workspace.name, "display,with,commas");
        assert!(workspace.special);
        for line in [
            "movewindowv2>>bad!,1,one",
            "movewindowv2>>123,1",
            "movewindowv2>>123,special:,name",
            "movewindowv2>>123,0,zero",
            "movewindowv2>>123,-2,negative",
            "movewindowv2>>123,18446744073709551615,overflow",
            "openwindow>>",
            "windowtitlev2>>123",
            "changeworkspaceid>>a,2",
        ] {
            assert!(
                matches!(
                    CompositorEvent::parse(line),
                    CompositorEvent::Malformed { .. }
                ),
                "{line}"
            );
        }
        assert_eq!(
            CompositorEvent::parse("windowtitlev2>>123,title,with,commas"),
            CompositorEvent::WindowTitleChanged
        );
        assert_eq!(
            CompositorEvent::parse("activewindowv2>>"),
            CompositorEvent::ActiveWindowChanged
        );
        let unknown = CompositorEvent::parse("future>>a,b,c");
        assert_eq!(
            unknown,
            CompositorEvent::Unknown {
                name: "future".into(),
                data: "a,b,c".into()
            }
        );
    }

    #[test]
    fn named_moves_keep_the_address_without_inventing_a_number() {
        for line in [
            "movewindow>>123,project",
            "movewindowv2>>123,project,display,with,commas",
        ] {
            let CompositorEvent::WindowMoved { workspace, .. } = CompositorEvent::parse(line)
            else {
                panic!("expected named move")
            };
            assert_eq!(workspace.id, None);
            assert_eq!(workspace.address.as_deref(), Some("project"));
            assert!(!workspace.special);
            assert_eq!(workspace.selector().unwrap(), "name:project");
        }
    }

    #[test]
    fn numeric_display_names_do_not_replace_workspace_addresses() {
        // 0.57.1 Window.cpp emits addressableName in both move forms; only v2
        // appends displayName. Workspace 7 may legitimately display as "6".
        for line in ["movewindow>>123,7", "movewindowv2>>123,7,6"] {
            let CompositorEvent::WindowMoved { workspace, .. } = CompositorEvent::parse(line)
            else {
                panic!("expected move");
            };
            assert_eq!(workspace.id, Some(7));
            assert!(!workspace.special);
        }
    }
}
