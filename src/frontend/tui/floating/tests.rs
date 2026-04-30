use super::*;

#[test]
fn anchored_area_prefers_below_and_right() {
    let area = resolve_anchored_area(
        Rect::new(0, 0, 40, 12),
        FloatingAnchor::new(2, 1),
        FloatingSize::new(22, 7),
    );

    assert_eq!(area, Rect::new(2, 2, 22, 7));
}

#[test]
fn anchored_area_flips_above_when_below_does_not_fit() {
    let area = resolve_anchored_area(
        Rect::new(0, 0, 40, 8),
        FloatingAnchor::new(2, 7),
        FloatingSize::new(22, 7),
    );

    assert_eq!(area, Rect::new(2, 0, 22, 7));
}

#[test]
fn anchored_area_flips_left_when_right_does_not_fit() {
    let area = resolve_anchored_area(
        Rect::new(0, 0, 26, 8),
        FloatingAnchor::new(22, 0),
        FloatingSize::new(22, 7),
    );

    assert_eq!(area, Rect::new(1, 1, 22, 7));
}

#[test]
fn anchored_area_clips_when_neither_side_can_fit() {
    let area = resolve_anchored_area(
        Rect::new(0, 0, 12, 4),
        FloatingAnchor::new(2, 1),
        FloatingSize::new(22, 7),
    );

    assert_eq!(area, Rect::new(2, 2, 10, 2));
}

#[test]
fn fixed_area_clips_to_bounds() {
    let area = resolve_fixed_area(Rect::new(5, 3, 10, 6), Rect::new(8, 4, 10, 4));

    assert_eq!(area, Rect::new(13, 7, 2, 2));
}

#[test]
fn floating_surface_clears_wide_character_crossing_left_edge() {
    let mut buffer = Buffer::with_lines(["中文中文中文", "中文中文中文", "中文中文中文"]);
    let surface = FloatingSurface {
        placement: FloatingPlacement::Anchored {
            anchor: FloatingAnchor::new(1, 0),
            size: FloatingSize::new(4, 2),
        },
        lines: vec![Line::raw("abcd"), Line::raw("efgh")],
    };

    surface.render(buffer.area, &mut buffer);

    assert_eq!(
        buffer[(0, 1)].symbol(),
        " ",
        "the wide glyph starting before the popup must be cleared because it occupies the popup's first cell"
    );
    assert_eq!(
        buffer[(1, 1)].symbol(),
        "a",
        "the popup text should start at the requested surface column"
    );
}

#[test]
fn floating_surface_keeps_single_width_cell_before_left_edge() {
    let mut buffer = Buffer::with_lines(["abcdef", "abcdef", "abcdef"]);
    let surface = FloatingSurface {
        placement: FloatingPlacement::Anchored {
            anchor: FloatingAnchor::new(2, 0),
            size: FloatingSize::new(3, 2),
        },
        lines: vec![Line::raw("xyz"), Line::raw("123")],
    };

    surface.render(buffer.area, &mut buffer);

    assert_eq!(
        buffer[(1, 1)].symbol(),
        "b",
        "single-width content immediately before the popup should stay outside the cleared area"
    );
    assert_eq!(buffer[(2, 1)].symbol(), "x");
}

#[test]
fn floating_surface_uses_rectangular_clear_when_left_edge_crosses_wide_character() {
    let mut buffer = Buffer::with_lines(["--------", "中文中文", "abcdefgh"]);
    let surface = FloatingSurface {
        placement: FloatingPlacement::Anchored {
            anchor: FloatingAnchor::new(1, 0),
            size: FloatingSize::new(4, 2),
        },
        lines: vec![Line::raw("wxyz"), Line::raw("1234")],
    };

    surface.render(buffer.area, &mut buffer);

    assert_eq!(
        buffer[(0, 1)].symbol(),
        " ",
        "the row with a wide character crossing the left edge should clear the glyph start"
    );
    assert_eq!(
        buffer[(0, 2)].symbol(),
        " ",
        "the safety clear should remain rectangular on rows that do not contain a wide character"
    );
}

#[test]
fn floating_surface_uses_rectangular_clear_when_right_edge_crosses_wide_character() {
    let mut buffer = Buffer::with_lines(["--------", "abcd中z", "abcdefg"]);
    let surface = FloatingSurface {
        placement: FloatingPlacement::Anchored {
            anchor: FloatingAnchor::new(1, 0),
            size: FloatingSize::new(4, 2),
        },
        lines: vec![Line::raw("wxyz"), Line::raw("1234")],
    };

    surface.render(buffer.area, &mut buffer);

    assert_eq!(
        buffer[(5, 1)].symbol(),
        " ",
        "the row with a wide character crossing the right edge already exposes a blank trailing cell"
    );
    assert_eq!(
        buffer[(5, 2)].symbol(),
        " ",
        "the safety clear should remain rectangular on rows that do not contain a wide character"
    );
}
