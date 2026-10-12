//! Windows monitor coordinates compatible with Chromium's DIP layout.
//! Physical monitors form a tree rooted at the primary display. Adjacent DIP
//! rectangles keep their shared edge even when their scale factors differ.
//! See Chromium's ui/display/win/scaling_util.cc and screen_win.cc.

use buddy_core::config::DisplayRect;

#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    pub id: u64,
    pub physical: DisplayRect,
    pub work: DisplayRect,
    pub scale: f64,
    pub dip: DisplayRect,
}

impl Monitor {
    pub fn new(id: u64, physical: DisplayRect, work: DisplayRect, scale: f64) -> Self {
        // Chromium encloses the scaled physical edges in integer DIP bounds.
        let x = (physical.x / scale).floor();
        let y = (physical.y / scale).floor();
        Self {
            id,
            physical,
            work,
            scale,
            dip: DisplayRect {
                x,
                y,
                width: ((physical.x + physical.width) / scale).ceil() - x,
                height: ((physical.y + physical.height) / scale).ceil() - y,
            },
        }
    }

    pub fn to_dip(self, rect: DisplayRect) -> DisplayRect {
        DisplayRect {
            x: self.dip.x + (rect.x - self.physical.x) / self.scale,
            y: self.dip.y + (rect.y - self.physical.y) / self.scale,
            width: rect.width / self.scale,
            height: rect.height / self.scale,
        }
    }

    pub fn to_physical(self, rect: DisplayRect) -> DisplayRect {
        DisplayRect {
            x: self.physical.x + (rect.x - self.dip.x) * self.scale,
            y: self.physical.y + (rect.y - self.dip.y) * self.scale,
            width: rect.width * self.scale,
            height: rect.height * self.scale,
        }
    }

    pub fn dip_work(self) -> DisplayRect {
        let left = ((self.work.x - self.physical.x) / self.scale).floor();
        let top = ((self.work.y - self.physical.y) / self.scale).floor();
        let right = ((self.work.x + self.work.width - self.physical.x) / self.scale).ceil();
        let bottom = ((self.work.y + self.work.height - self.physical.y) / self.scale).ceil();
        DisplayRect {
            x: self.dip.x + left,
            y: self.dip.y + top,
            width: right - left,
            height: bottom - top,
        }
    }
}

pub fn layout(monitors: &mut [Monitor]) {
    let Some(primary) = monitors
        .iter()
        .position(|m| m.physical.x == 0.0 && m.physical.y == 0.0)
    else {
        return;
    };
    let mut placed = vec![false; monitors.len()];
    placed[primary] = true;
    let mut parents = vec![primary];
    while let Some(parent) = parents.pop() {
        for child in 0..monitors.len() {
            if placed[child] {
                continue;
            }
            if let Some((x, y)) = adjacent_origin(monitors[parent], monitors[child]) {
                monitors[child].dip.x = x;
                monitors[child].dip.y = y;
                placed[child] = true;
                parents.push(child);
            }
        }
    }
}

fn adjacent_origin(parent: Monitor, child: Monitor) -> Option<(f64, f64)> {
    let a = parent.physical;
    let b = child.physical;
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    let vertical = left == right && top <= bottom;
    let horizontal = top == bottom && left <= right;
    if !vertical && !horizontal {
        return None;
    }
    // At a corner Chromium chooses bottom, then left, then top.
    let side = if vertical && horizontal {
        if a.y + a.height == top {
            2
        } else if a.x == left {
            3
        } else {
            0
        }
    } else if vertical {
        if a.x == left { 3 } else { 1 }
    } else if a.y == top {
        0
    } else {
        2
    };
    let (begin, length, child_begin, child_length, dip_length, child_dip_length) =
        if side == 0 || side == 2 {
            (
                a.x,
                a.width,
                b.x,
                b.width,
                parent.dip.width,
                child.dip.width,
            )
        } else {
            (
                a.y,
                a.height,
                b.y,
                b.height,
                parent.dip.height,
                child.dip.height,
            )
        };
    let relative = child_begin - begin;
    let end = relative + child_length;
    let offset = if end == length && relative != 0.0 {
        dip_length - child_dip_length
    } else if (0.0..=length).contains(&relative) {
        (relative / parent.scale).floor()
    } else if (0.0..=length).contains(&end) {
        dip_length - ((length - end) / parent.scale).floor() - child_dip_length
    } else {
        (relative / child.scale).floor()
    };
    let offset = offset.clamp(-child_dip_length, dip_length);
    let p = parent.dip;
    Some(match side {
        0 => (p.x + offset, p.y - child.dip.height),
        1 => (p.x + p.width, p.y + offset),
        2 => (p.x + offset, p.y + p.height),
        _ => (p.x - child.dip.width, p.y + offset),
    })
}

pub fn matching(monitors: &[Monitor], rect: DisplayRect) -> Option<Monitor> {
    monitors
        .iter()
        .copied()
        .map(|m| {
            let d = m.dip;
            let width = ((rect.x + rect.width).min(d.x + d.width) - rect.x.max(d.x)).max(0.0);
            let height = ((rect.y + rect.height).min(d.y + d.height) - rect.y.max(d.y)).max(0.0);
            (m, width * height)
        })
        .filter(|(_, overlap)| *overlap > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(m, _)| m)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: f64, y: f64, width: f64, height: f64) -> DisplayRect {
        DisplayRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn mixed_dpi_displays_share_the_same_edge_as_electron() {
        let mut monitors = [
            Monitor::new(
                1,
                rect(0.0, 0.0, 3440.0, 1440.0),
                rect(0.0, 0.0, 3440.0, 1392.0),
                1.0,
            ),
            Monitor::new(
                2,
                rect(3440.0, 0.0, 3840.0, 2160.0),
                rect(3440.0, 0.0, 3840.0, 2076.0),
                1.75,
            ),
        ];
        layout(&mut monitors);
        assert_eq!(monitors[1].dip, rect(3440.0, 0.0, 2195.0, 1235.0));
        assert_eq!(monitors[1].dip_work(), rect(3440.0, 0.0, 2195.0, 1187.0));
        let saved = monitors[1].to_dip(rect(3574.0, 90.0, 1652.0, 1246.0));
        assert_eq!((saved.width, saved.height), (944.0, 712.0));
        assert!((saved.x - 3516.5714285714284).abs() < 0.00001);
        assert_eq!(matching(&monitors, saved).map(|m| m.id), Some(2));
        assert_eq!(
            monitors[1].to_physical(saved),
            rect(3574.0, 90.0, 1652.0, 1246.0)
        );
    }

    #[test]
    fn left_top_and_bottom_displays_and_end_alignment_are_scaled() {
        for (physical, expected) in [
            (
                rect(-3840.0, 0.0, 3840.0, 2160.0),
                rect(-2195.0, 0.0, 2195.0, 1235.0),
            ),
            (
                rect(0.0, -2160.0, 3840.0, 2160.0),
                rect(0.0, -1235.0, 2195.0, 1235.0),
            ),
            (
                rect(0.0, 1440.0, 3840.0, 2160.0),
                rect(0.0, 1440.0, 2195.0, 1236.0),
            ),
            (
                rect(3440.0, -720.0, 3840.0, 2160.0),
                rect(3440.0, 205.0, 2195.0, 1235.0),
            ),
        ] {
            let p = rect(0.0, 0.0, 3440.0, 1440.0);
            let mut monitors = [
                Monitor::new(1, p, p, 1.0),
                Monitor::new(2, physical, physical, 1.75),
            ];
            layout(&mut monitors);
            assert_eq!(monitors[1].dip, expected);
        }
    }
}
