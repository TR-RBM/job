use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::composite::{self, ConnectionExt as _};
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::res::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _, ImageFormat, Rectangle, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub const DEFAULT_DISPLAY: &str = ":0";

pub const REPAINT_WAIT: Duration = Duration::from_secs(10);

const FULL_REPAINTS: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub primary: bool,
}

impl Monitor {
    pub fn geometry(&self) -> String {
        format!("{}x{}+{}+{}", self.width, self.height, self.x, self.y)
    }
}

pub struct Display {
    connection: RustConnection,
    root: Window,
    pub name: String,
}

fn failed(what: &str, e: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("{what}: {e}"))
}

pub fn open(name: Option<&str>) -> io::Result<Display> {
    let name = name
        .map(str::to_string)
        .or_else(|| std::env::var("DISPLAY").ok())
        .unwrap_or_else(|| DEFAULT_DISPLAY.to_string());
    let (connection, screen) = x11rb::connect(Some(&name))
        .map_err(|e| failed(&format!("cannot open the display {name}"), e))?;
    let root = connection.setup().roots[screen].root;
    Ok(Display {
        connection,
        root,
        name,
    })
}

impl Display {
    pub fn monitors(&self) -> io::Result<Vec<Monitor>> {
        let reply = self
            .connection
            .randr_get_monitors(self.root, true)
            .map_err(|e| failed("RandR", e))?
            .reply()
            .map_err(|e| failed("RandR 1.5 monitors", e))?;
        let mut monitors = Vec::new();
        for info in reply.monitors {
            let name = self
                .connection
                .get_atom_name(info.name)
                .map_err(|e| failed("atom", e))?
                .reply()
                .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                .unwrap_or_default();
            monitors.push(Monitor {
                name,
                x: info.x,
                y: info.y,
                width: info.width,
                height: info.height,
                primary: info.primary,
            });
        }
        Ok(monitors)
    }

    fn image(&self, drawable: u32, x: i16, y: i16, width: u16, height: u16) -> io::Result<Vec<u8>> {
        let reply = self
            .connection
            .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, width, height, !0)
            .map_err(|e| failed("GetImage", e))?
            .reply()
            .map_err(|e| failed("GetImage", e))?;
        let pixels = width as usize * height as usize;
        if reply.data.len() < pixels * 4 {
            return Err(io::Error::other(format!(
                "the display returned {} bytes for {pixels} pixels at depth {}; only 24- and 32-bit visuals are read",
                reply.data.len(),
                reply.depth
            )));
        }
        let mut rgb = Vec::with_capacity(pixels * 3);
        for pixel in reply.data.as_chunks::<4>().0.iter().take(pixels) {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
        Ok(rgb)
    }

    pub fn capture_monitor(&self, monitor: &Monitor, path: &Path) -> io::Result<()> {
        let rgb = self.image(
            self.root,
            monitor.x,
            monitor.y,
            monitor.width,
            monitor.height,
        )?;
        write_png(path, monitor.width, monitor.height, &rgb)
    }

    fn property_cardinal(&self, window: Window, atom: u32) -> Option<u32> {
        let reply = self
            .connection
            .get_property(false, window, atom, xproto::AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        reply.value32()?.next()
    }

    fn atom(&self, name: &str) -> io::Result<u32> {
        Ok(self
            .connection
            .intern_atom(false, name.as_bytes())
            .map_err(|e| failed("atom", e))?
            .reply()
            .map_err(|e| failed("atom", e))?
            .atom)
    }

    fn client_pids(&self) -> Vec<(u32, i32)> {
        let spec = res::ClientIdSpec {
            client: 0,
            mask: res::ClientIdMask::LOCAL_CLIENT_PID,
        };
        self.connection
            .res_query_client_ids(&[spec])
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|reply| {
                reply
                    .ids
                    .into_iter()
                    .filter_map(|id| Some((id.spec.client, *id.value.first()? as i32)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn windows_of(&self, pids: &[i32]) -> io::Result<Vec<Window>> {
        let pid_atom = self.atom("_NET_WM_PID")?;
        let base_mask = !self.connection.setup().resource_id_mask;
        let owners = self.client_pids();
        let owner_of = |window: Window| {
            self.property_cardinal(window, pid_atom)
                .map(|pid| pid as i32)
                .or_else(|| {
                    owners
                        .iter()
                        .find(|(base, _)| *base == window & base_mask)
                        .map(|(_, pid)| *pid)
                })
        };
        let mut found = Vec::new();
        let mut pending = vec![(self.root, 0u8)];
        while let Some((window, depth)) = pending.pop() {
            if window != self.root
                && let Some(pid) = owner_of(window)
                && pids.contains(&pid)
            {
                let viewable = self
                    .connection
                    .get_window_attributes(window)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .is_some_and(|a| a.map_state == xproto::MapState::VIEWABLE);
                if viewable {
                    found.push(window);
                }
                continue;
            }
            if depth < 3
                && let Ok(tree) = self
                    .connection
                    .query_tree(window)
                    .map_err(|e| failed("QueryTree", e))?
                    .reply()
            {
                pending.extend(tree.children.into_iter().map(|c| (c, depth + 1)));
            }
        }
        Ok(found)
    }

    pub fn position_of(&self, window: Window) -> io::Result<(i16, i16, u16, u16)> {
        let geometry = self
            .connection
            .get_geometry(window)
            .map_err(|e| failed("GetGeometry", e))?
            .reply()
            .map_err(|e| failed("GetGeometry", e))?;
        let origin = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)
            .map_err(|e| failed("TranslateCoordinates", e))?
            .reply()
            .map_err(|e| failed("TranslateCoordinates", e))?;
        Ok((origin.dst_x, origin.dst_y, geometry.width, geometry.height))
    }

    pub fn move_window(&self, window: Window, x: i16, y: i16) -> io::Result<()> {
        self.connection
            .configure_window(
                window,
                &xproto::ConfigureWindowAux::new()
                    .x(i32::from(x))
                    .y(i32::from(y)),
            )
            .map_err(|e| failed("ConfigureWindow", e))?;
        self.connection.flush().map_err(|e| failed("flush", e))
    }

    pub fn capture_window(&self, window: Window, path: &Path) -> io::Result<(u16, u16)> {
        self.require_extension("Composite", self.connection.composite_query_version(0, 4))?;
        self.require_extension("XFixes", self.connection.xfixes_query_version(5, 0))?;
        self.require_extension("Damage", self.connection.damage_query_version(1, 1))?;
        self.connection
            .composite_redirect_window(window, composite::Redirect::AUTOMATIC)
            .map_err(|e| failed("Composite", e))?;
        let watched = self.watch(window).and_then(|damage| {
            let captured = self
                .await_repaint(window, damage)
                .and_then(|(width, height)| {
                    let rgb = self.window_pixmap_image(window, width, height)?;
                    Ok((width, height, rgb))
                });
            let _ = self.connection.damage_destroy(damage);
            captured
        });
        let _ = self
            .connection
            .composite_unredirect_window(window, composite::Redirect::AUTOMATIC);
        let _ = self.connection.sync();
        let (width, height, rgb) = watched?;
        write_png(path, width, height, &rgb)?;
        Ok((width, height))
    }

    fn require_extension<R>(
        &self,
        name: &str,
        cookie: Result<
            x11rb::cookie::Cookie<'_, RustConnection, R>,
            x11rb::errors::ConnectionError,
        >,
    ) -> io::Result<()>
    where
        R: x11rb::x11_utils::TryParse,
    {
        cookie.ok().and_then(|c| c.reply().ok()).map(|_| ()).ok_or_else(|| {
            io::Error::other(format!(
                "the display has no {name} extension, so a window's own pixels cannot be told apart from the screen beneath it; nothing was written"
            ))
        })
    }

    fn watch(&self, window: Window) -> io::Result<damage::Damage> {
        self.connection
            .change_window_attributes(
                window,
                &xproto::ChangeWindowAttributesAux::new()
                    .event_mask(xproto::EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(|e| failed("ChangeWindowAttributes", e))?;
        let damage = self.connection.generate_id().map_err(|e| failed("id", e))?;
        self.connection
            .damage_create(damage, window, damage::ReportLevel::RAW_RECTANGLES)
            .map_err(|e| failed("Damage", e))?;
        self.connection.sync().map_err(|e| failed("sync", e))?;
        Ok(damage)
    }

    fn await_repaint(&self, window: Window, damage: damage::Damage) -> io::Result<(u16, u16)> {
        let geometry = self
            .connection
            .get_geometry(window)
            .map_err(|e| failed("GetGeometry", e))?
            .reply()
            .map_err(|e| failed("GetGeometry", e))?;
        let mut repaint = Repaint::new(geometry.width, geometry.height);
        let deadline = Instant::now() + REPAINT_WAIT;
        while Instant::now() < deadline {
            while let Some(event) = self
                .connection
                .poll_for_event()
                .map_err(|e| failed("event", e))?
            {
                match event {
                    Event::DamageNotify(notify) if notify.damage == damage => {
                        repaint.painted(notify.area)
                    }
                    Event::ConfigureNotify(e) if e.window == window => {
                        repaint.resized(e.width, e.height)
                    }
                    Event::MapNotify(e) if e.window == window => repaint.restart(),
                    Event::UnmapNotify(e) if e.window == window => repaint.restart(),
                    _ => {}
                }
                if repaint.complete() {
                    return Ok(repaint.size());
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(io::Error::other(format!(
            "window 0x{window:x} did not repaint all of itself {FULL_REPAINTS} times within {} s after the capture began ({}), so its pixels could still be the screen beneath it; nothing was written. Capture it while it presents frames, or read the program's own framebuffer",
            REPAINT_WAIT.as_secs(),
            repaint.progress()
        )))
    }

    fn window_pixmap_image(&self, window: Window, width: u16, height: u16) -> io::Result<Vec<u8>> {
        let pixmap = self.connection.generate_id().map_err(|e| failed("id", e))?;
        self.connection
            .composite_name_window_pixmap(window, pixmap)
            .map_err(|e| failed("Composite", e))?;
        let image = self.image(pixmap, 0, 0, width, height);
        let _ = self.connection.free_pixmap(pixmap);
        image
    }
}

struct Repaint {
    width: u16,
    height: u16,
    painted: Vec<bool>,
    count: usize,
    full: u8,
}

impl Repaint {
    fn new(width: u16, height: u16) -> Self {
        Repaint {
            width,
            height,
            painted: vec![false; usize::from(width) * usize::from(height)],
            count: 0,
            full: 0,
        }
    }

    fn restart(&mut self) {
        *self = Repaint::new(self.width, self.height);
    }

    fn resized(&mut self, width: u16, height: u16) {
        if (width, height) != (self.width, self.height) {
            *self = Repaint::new(width, height);
        }
    }

    fn painted(&mut self, area: Rectangle) {
        let clip = |start: i16, extent: u16, limit: u16| {
            let from = i32::from(start).clamp(0, i32::from(limit));
            let to = (i32::from(start) + i32::from(extent)).clamp(0, i32::from(limit));
            (from as usize, to as usize)
        };
        let (left, right) = clip(area.x, area.width, self.width);
        let (top, bottom) = clip(area.y, area.height, self.height);
        for row in top..bottom {
            let line = row * usize::from(self.width);
            for cell in &mut self.painted[line + left..line + right] {
                if !*cell {
                    *cell = true;
                    self.count += 1;
                }
            }
        }
        if self.count == self.painted.len() && !self.painted.is_empty() {
            self.full += 1;
            self.painted.fill(false);
            self.count = 0;
        }
    }

    fn complete(&self) -> bool {
        self.full >= FULL_REPAINTS
    }

    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    fn progress(&self) -> String {
        format!(
            "{} full repaints, then {} of {} pixels",
            self.full,
            self.count,
            self.painted.len()
        )
    }
}

pub fn monitor_at(monitors: &[Monitor], x: i16, y: i16) -> Option<&Monitor> {
    monitors.iter().find(|m| {
        x >= m.x
            && y >= m.y
            && i32::from(x) < i32::from(m.x) + i32::from(m.width)
            && i32::from(y) < i32::from(m.y) + i32::from(m.height)
    })
}

pub fn choose<'a>(monitors: &'a [Monitor], wanted: &str) -> Result<&'a Monitor, String> {
    let names = || {
        monitors
            .iter()
            .map(|m| format!("{} {}", m.name, m.geometry()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let by_position = |leftmost: bool| {
        monitors.iter().min_by_key(|m| {
            let x = i32::from(m.x);
            (if leftmost { x } else { -x }, i32::from(m.y))
        })
    };
    let found = match wanted {
        "left" => by_position(true),
        "right" => by_position(false),
        "primary" => monitors.iter().find(|m| m.primary),
        _ => match wanted.parse::<usize>() {
            Ok(index) => monitors.get(index),
            Err(_) => monitors
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(wanted)),
        },
    };
    found.ok_or_else(|| {
        format!("no monitor `{wanted}`; the display has {}; name one of them, or left, right, primary or its number", names())
    })
}

pub fn write_png(path: &Path, width: u16, height: u16, rgb: &[u8]) -> io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(
        io::BufWriter::new(file),
        u32::from(width),
        u32::from(height),
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| failed("PNG", e))?;
    writer.write_image_data(rgb).map_err(|e| failed("PNG", e))?;
    writer.finish().map_err(|e| failed("PNG", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, x: i16, primary: bool) -> Monitor {
        Monitor {
            name: name.to_string(),
            x,
            y: 0,
            width: 1920,
            height: 1080,
            primary,
        }
    }

    fn rectangle(x: i16, y: i16, width: u16, height: u16) -> Rectangle {
        Rectangle {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_window_counts_as_repainted_after_two_full_covers() {
        let mut repaint = Repaint::new(4, 3);
        repaint.painted(rectangle(0, 0, 4, 3));
        assert!(!repaint.complete());
        repaint.painted(rectangle(0, 0, 4, 3));
        assert!(repaint.complete());
    }

    #[test]
    fn pieces_that_together_cover_the_window_count_once_each_pixel() {
        let mut repaint = Repaint::new(4, 2);
        repaint.painted(rectangle(0, 0, 3, 2));
        repaint.painted(rectangle(1, 0, 2, 2));
        assert_eq!(repaint.full, 0);
        repaint.painted(rectangle(3, 0, 1, 2));
        assert_eq!(repaint.full, 1);
    }

    #[test]
    fn damage_outside_the_window_covers_nothing() {
        let mut repaint = Repaint::new(4, 2);
        repaint.painted(rectangle(-10, -10, 20, 5));
        repaint.painted(rectangle(4, 0, 10, 2));
        assert_eq!((repaint.full, repaint.count), (0, 0));
        repaint.painted(rectangle(-2, -2, 20, 20));
        assert_eq!(repaint.full, 1);
    }

    #[test]
    fn a_resize_or_a_new_mapping_starts_the_count_again() {
        let mut repaint = Repaint::new(4, 2);
        repaint.painted(rectangle(0, 0, 4, 2));
        repaint.resized(4, 2);
        assert_eq!(repaint.full, 1);
        repaint.resized(5, 2);
        assert_eq!((repaint.full, repaint.size()), (0, (5, 2)));
        repaint.painted(rectangle(0, 0, 5, 2));
        repaint.restart();
        assert_eq!(repaint.full, 0);
    }

    #[test]
    fn a_window_without_pixels_is_never_repainted() {
        let mut repaint = Repaint::new(0, 0);
        repaint.painted(rectangle(0, 0, 10, 10));
        assert!(!repaint.complete());
    }

    #[test]
    fn a_monitor_is_chosen_by_name_position_primary_or_number() {
        let monitors = vec![monitor("DP-4", 1920, false), monitor("HDMI-0", 0, true)];
        assert_eq!(choose(&monitors, "left").unwrap().name, "HDMI-0");
        assert_eq!(choose(&monitors, "right").unwrap().name, "DP-4");
        assert_eq!(choose(&monitors, "primary").unwrap().name, "HDMI-0");
        assert_eq!(choose(&monitors, "hdmi-0").unwrap().name, "HDMI-0");
        assert_eq!(choose(&monitors, "0").unwrap().name, "DP-4");
        let refused = choose(&monitors, "middle").unwrap_err();
        assert!(refused.contains("HDMI-0 1920x1080+0+0"), "{refused}");
    }
}
