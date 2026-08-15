use ratatui::style::Style;

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
fn vertical_anchor_opens_above_matches_resolve_vertical_axis() {
    let cases = [
        (Rect::new(0, 0, 40, 12), 1, 7, false),
        (Rect::new(0, 0, 40, 8), 7, 7, true),
        (Rect::new(0, 0, 12, 4), 1, 7, false),
    ];

    for (bounds, relative_anchor_y, height, expected_above) in cases {
        let anchor_y = bounds.y.saturating_add(relative_anchor_y);
        assert_eq!(
            vertical_anchor_opens_above(bounds, anchor_y, height),
            expected_above,
            "chrome order must use the same above/below decision as placement: bounds={bounds:?} anchor_y={anchor_y} height={height}"
        );

        let (resolved_y, resolved_height) = resolve_vertical_axis(bounds, anchor_y, height);
        if expected_above {
            assert!(
                resolved_y.saturating_add(resolved_height) <= anchor_y,
                "opening above should keep the popup at or above the anchor: y={resolved_y} height={resolved_height} anchor_y={anchor_y}"
            );
        } else {
            assert!(
                resolved_y > anchor_y || resolved_height == 0,
                "opening below should start after the anchor: y={resolved_y} height={resolved_height} anchor_y={anchor_y}"
            );
        }
    }
}

#[test]
fn floating_surface_keeps_scrollbar_on_offset_list_track() {
    let mut buffer = Buffer::with_lines([
        "........", "........", "........", "........", "........", "........", "........",
        "........",
    ]);
    let surface = FloatingSurface {
        placement: FloatingPlacement::Anchored {
            anchor: FloatingAnchor::new(0, 0),
            size: FloatingSize::new(8, 5),
        },
        lines: vec![
            Line::raw("footer  "),
            Line::raw("        "),
            Line::raw("item0   "),
            Line::raw("item1   "),
            Line::raw("item2   "),
        ],
        scrollbar: Some(PickerScrollbar::new(
            6,
            3,
            0,
            Style::default(),
            Style::default(),
        )),
        scrollbar_track_height: Some(3),
        scrollbar_track_y_offset: 2,
    };

    surface.render(buffer.area, &mut buffer);

    let scrollbar_column = 7;
    assert!(
        !matches!(buffer[(scrollbar_column, 1)].symbol(), "█" | "┃"),
        "leading footer chrome should stay outside the scrollbar track: {:?}",
        buffer[(scrollbar_column, 1)].symbol()
    );
    assert!(
        !matches!(buffer[(scrollbar_column, 2)].symbol(), "█" | "┃"),
        "the blank chrome row should stay outside the scrollbar track: {:?}",
        buffer[(scrollbar_column, 2)].symbol()
    );
    assert!(
        matches!(buffer[(scrollbar_column, 3)].symbol(), "█" | "┃"),
        "the scrollbar track should start on the first list row: {:?}",
        buffer[(scrollbar_column, 3)].symbol()
    );
    assert!(
        matches!(buffer[(scrollbar_column, 5)].symbol(), "█" | "┃"),
        "the scrollbar track should cover the last list row: {:?}",
        buffer[(scrollbar_column, 5)].symbol()
    );
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
        scrollbar: None,
        scrollbar_track_height: None,
        scrollbar_track_y_offset: 0,
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
        scrollbar: None,
        scrollbar_track_height: None,
        scrollbar_track_y_offset: 0,
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
        scrollbar: None,
        scrollbar_track_height: None,
        scrollbar_track_y_offset: 0,
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
        scrollbar: None,
        scrollbar_track_height: None,
        scrollbar_track_y_offset: 0,
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
