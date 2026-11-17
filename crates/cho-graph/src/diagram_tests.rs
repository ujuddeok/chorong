//! Diagram layout and SVG: deterministic, no overlaps, routes that meet their boxes.

use super::*;
use crate::tests::{fixture, id, spec};

fn node(id: &str, group: Option<&str>) -> DiagramNode {
    DiagramNode {
        id: id.into(),
        label: id.into(),
        group: group.map(Into::into),
        ..Default::default()
    }
}

fn edge(from: &str, to: &str, line: LineKind) -> DiagramEdge {
    DiagramEdge {
        from: from.into(),
        to: to.into(),
        line,
        label: None,
    }
}

/// A tour-sized diagram: a fan out, a long edge, a cycle, a self loop, two groups and an
/// edge to a node that does not exist.
fn sample(orientation: Orientation) -> Diagram {
    let mut nodes = vec![
        node("Server::handle", Some("server.rs")),
        node("Job::from", Some("job.rs")),
        node("Queue::push", Some("queue.rs")),
        node("Server::wake", Some("server.rs")),
        node("Condvar::notify_one", None),
        node("Job::validate", Some("job.rs")),
    ];
    nodes[0].style = NodeStyle::Focus;
    nodes[4].style = NodeStyle::External;
    let mut edges = vec![
        edge("Server::handle", "Job::from", LineKind::Verified),
        edge("Server::handle", "Queue::push", LineKind::Verified),
        edge("Server::handle", "Server::wake", LineKind::Claimed),
        edge("Server::wake", "Condvar::notify_one", LineKind::Verified),
        edge("Job::from", "Job::validate", LineKind::Verified),
        edge("Server::handle", "Condvar::notify_one", LineKind::Broken),
        edge("Job::validate", "Server::handle", LineKind::Claimed),
        edge("Queue::push", "Queue::push", LineKind::Verified),
        edge("Queue::push", "nowhere", LineKind::Verified),
    ];
    edges[3].label = Some("calls".into());
    Diagram {
        nodes,
        edges,
        orientation,
        ..Default::default()
    }
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn on_border(r: &Rect, p: (i32, i32)) -> bool {
    let inside_x = p.0 >= r.x && p.0 <= r.x + r.w;
    let inside_y = p.1 >= r.y && p.1 <= r.y + r.h;
    (inside_x && (p.1 == r.y || p.1 == r.y + r.h)) || (inside_y && (p.0 == r.x || p.0 == r.x + r.w))
}

fn check(d: &Diagram) -> Layout {
    let l = layout(d);
    assert_eq!(
        l.dropped_edges,
        vec![8],
        "the edge to a missing node is dropped"
    );
    assert_eq!(l.nodes.len(), 6);
    for (i, a) in l.nodes.iter().enumerate() {
        assert!(a.rect.x >= 0 && a.rect.y >= 0);
        assert!(a.rect.x + a.rect.w <= l.width && a.rect.y + a.rect.h <= l.height);
        for b in &l.nodes[i + 1..] {
            assert!(!overlaps(&a.rect, &b.rect), "{} overlaps {}", a.id, b.id);
        }
    }
    for (i, a) in l.groups.iter().enumerate() {
        for b in &l.groups[i + 1..] {
            assert!(
                !overlaps(&a.rect, &b.rect),
                "groups {} and {}",
                a.label,
                b.label
            );
        }
    }
    // Members sit inside their group's box; nothing else does.
    for nb in &l.nodes {
        let g = d.nodes[nb.index].group.as_deref();
        for gb in &l.groups {
            let r = &gb.rect;
            let inside = nb.rect.x >= r.x
                && nb.rect.y >= r.y
                && nb.rect.x + nb.rect.w <= r.x + r.w
                && nb.rect.y + nb.rect.h <= r.y + r.h;
            assert_eq!(
                inside,
                g == Some(gb.label.as_str()),
                "{} in {}",
                nb.id,
                gb.label
            );
            if g != Some(gb.label.as_str()) {
                assert!(
                    !overlaps(&nb.rect, r),
                    "{} overlaps group {}",
                    nb.id,
                    gb.label
                );
            }
        }
    }
    let rect = |id: &str| l.nodes.iter().find(|n| n.id == id).unwrap().rect;
    assert_eq!(l.edges.len(), 8);
    for r in &l.edges {
        let e = &d.edges[r.index];
        assert_eq!(r.line, e.line);
        let (first, last) = (r.points[0], *r.points.last().unwrap());
        assert!(
            on_border(&rect(&e.from), first),
            "{:?} starts on {}",
            r.points,
            e.from
        );
        assert!(
            on_border(&rect(&e.to), last),
            "{:?} ends on {}",
            r.points,
            e.to
        );
        for w in r.points.windows(2) {
            assert!(
                w[0].0 == w[1].0 || w[0].1 == w[1].1,
                "orthogonal: {:?}",
                r.points
            );
        }
    }
    assert!(
        l.edges.iter().any(|r| r.reversed),
        "the cycle reverses one edge"
    );
    assert!(l.edges.iter().any(|r| r.label_rect.is_some()));
    l
}

#[test]
fn layouts_are_sound_in_both_orientations() {
    let td = check(&sample(Orientation::TopDown));
    let lr = check(&sample(Orientation::LeftRight));
    // Edges run with the orientation: the focus is above (or left of) what it calls.
    let at = |l: &Layout, id: &str| l.nodes.iter().find(|n| n.id == id).unwrap().rect;
    assert!(at(&td, "Server::handle").y < at(&td, "Job::from").y);
    assert!(at(&lr, "Server::handle").x < at(&lr, "Job::from").x);
    assert_eq!(td.groups.len(), 3);
}

#[test]
fn same_input_gives_byte_identical_layout_and_svg() {
    for o in [Orientation::TopDown, Orientation::LeftRight] {
        let d = sample(o);
        let a = layout(&d);
        let b = layout(&d.clone());
        assert_eq!(a, b);
        let theme = cho_theme::Theme::bundled(cho_theme::Appearance::Dark);
        let (sa, sb) = (to_svg(&d, &a, Some(&theme)), to_svg(&d, &b, Some(&theme)));
        assert_eq!(sa, sb);
        assert_eq!(to_svg(&d, &a, None), to_svg(&d, &b, None));
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}

#[test]
fn svg_has_real_text_theme_variables_and_readable_sizes() {
    let mut d = sample(Orientation::LeftRight);
    d.nodes[1].label = "A<b>&\"c\"".into();
    d.nodes[1].link = Some("chorong://open?path=a&line=2".into());
    d.nodes[1].detail = Some("src/job.rs:12".into());
    let l = layout(&d);
    let svg = to_svg(&d, &l, None);
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(svg.contains(">A&lt;b&gt;&amp;&quot;c&quot;</text>"));
    assert!(svg.contains("<a href=\"chorong://open?path=a&amp;line=2\">"));
    assert!(svg.contains("<title>src/job.rs:12</title>"));
    assert!(svg.contains("font-size:12px"));
    assert!(svg.contains("prefers-color-scheme: dark"));
    assert!(svg.contains("--verified:") && svg.contains("--claimed:") && svg.contains("--broken:"));
    assert!(svg.contains("class=\"edge claimed\"") && svg.contains("class=\"edge broken\""));
    assert!(svg.contains("class=\"node focus\"") && svg.contains("class=\"node external\""));
    // No sizes below 12 px anywhere.
    for (i, _) in svg.match_indices("font-size:") {
        let n: String = svg[i + 10..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        assert!(n.parse::<u32>().unwrap() >= 12);
    }
    let fixed = to_svg(
        &d,
        &l,
        Some(&cho_theme::Theme::bundled(cho_theme::Appearance::Light)),
    );
    assert!(!fixed.contains("prefers-color-scheme"));
}

#[test]
fn empty_and_tiny_diagrams_lay_out() {
    let l = layout(&Diagram::default());
    assert!(l.nodes.is_empty() && l.width > 0 && l.height > 0);
    let one = Diagram {
        nodes: vec![node("only", None)],
        ..Default::default()
    };
    let l = layout(&one);
    assert_eq!(l.nodes.len(), 1);
    assert!(
        l.node_at(l.nodes[0].rect.x + 2, l.nodes[0].rect.y + 2)
            .is_some()
    );
    assert!(l.node_at(0, 0).is_none());
    // Duplicate ids keep the first.
    let dup = Diagram {
        nodes: vec![node("a", None), node("a", Some("g")), node("b", None)],
        edges: vec![edge("a", "b", LineKind::Verified)],
        ..Default::default()
    };
    let l = layout(&dup);
    assert_eq!(l.nodes.len(), 2);
    assert!(l.groups.is_empty());
}

#[test]
fn long_labels_are_cut() {
    let d = Diagram {
        nodes: vec![node(&"x".repeat(100), None)],
        ..Default::default()
    };
    let l = layout(&d);
    assert_eq!(l.nodes[0].label.chars().count(), MAX_LABEL_CHARS);
    assert!(l.nodes[0].label.ends_with('\u{2026}'));
}

#[test]
fn a_neighbourhood_from_the_index_becomes_a_diagram() {
    let (_fx, sem) = fixture("diagram");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.direction = Direction::Out;
    s.depth = 2;
    s.externals = true;
    let graph = neighbourhood(&g, &[id(&g, "handle")], &s);
    let d = diagram_from_graph(&graph, GroupBy::Package, Orientation::LeftRight);
    let focus = d
        .nodes
        .iter()
        .find(|n| n.style == NodeStyle::Focus)
        .unwrap();
    assert_eq!(
        focus.label,
        graph.nodes.iter().find(|n| n.focus).unwrap().label
    );
    assert!(d.nodes.iter().all(|n| graph.node(&n.id).is_some()));
    assert!(
        d.nodes
            .iter()
            .all(|n| graph.node(&n.id).unwrap().kind != NodeKind::File)
    );
    assert!(d.edges.iter().all(|e| e.line == LineKind::Verified));
    assert!(d.nodes.iter().any(|n| n.group.as_deref() == Some("core")));
    let l = layout(&d);
    assert_eq!(l.edges.len(), d.edges.len());
    assert_eq!(layout(&d), l);
    let again = diagram_from_graph(
        &neighbourhood(&g, &[id(&g, "handle")], &s),
        GroupBy::Package,
        Orientation::LeftRight,
    );
    assert_eq!(again, d);
}

#[test]
fn neighbourhood_diagrams_hide_tests_and_say_so() {
    let (_fx, sem) = fixture("diagram-tests");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.direction = Direction::In;
    s.depth = 2;
    let graph = neighbourhood(&g, &[id(&g, "create_session")], &s);
    assert!(
        graph
            .nodes
            .iter()
            .any(|n| n.is_test && n.kind == NodeKind::Symbol)
    );
    let d = diagram_from_graph(&graph, GroupBy::None, Orientation::TopDown);
    assert!(d.nodes.iter().all(|n| !graph.node(&n.id).unwrap().is_test));
    assert_eq!(d.notes, vec!["1 test hidden".to_string()]);
    assert!(
        d.edges.iter().all(|e| e.label.is_none()),
        "calls and references carry no label"
    );
    // Every edge kept joins two different walk distances.
    for e in &d.edges {
        let (a, b) = (graph.node(&e.from).unwrap(), graph.node(&e.to).unwrap());
        assert_ne!(a.depth, b.depth, "{} -> {}", e.from, e.to);
    }
}

/// A fan out of five and a fan in of three, some edges labelled.
fn fans(orientation: Orientation) -> Diagram {
    let mut nodes = vec![node("focus", None)];
    nodes[0].style = NodeStyle::Focus;
    let mut edges = vec![];
    for i in 0..5 {
        nodes.push(node(&format!("callee_{i}"), None));
        let mut e = edge("focus", &format!("callee_{i}"), LineKind::Verified);
        if i % 2 == 0 {
            e.label = Some("type-uses".into());
        }
        edges.push(e);
    }
    for i in 0..3 {
        nodes.push(node(&format!("caller_{i}"), None));
        edges.push(edge(&format!("caller_{i}"), "focus", LineKind::Claimed));
    }
    Diagram {
        nodes,
        edges,
        orientation,
        ..Default::default()
    }
}

#[test]
fn fans_share_one_trunk_and_labels_stay_clear() {
    for o in [Orientation::LeftRight, Orientation::TopDown] {
        let d = fans(o);
        let l = layout(&d);
        assert_eq!(l, layout(&d.clone()), "deterministic");
        let theme = cho_theme::Theme::bundled(cho_theme::Appearance::Light);
        assert_eq!(to_svg(&d, &l, Some(&theme)), to_svg(&d, &l, Some(&theme)));
        let rect = |id: &str| l.nodes.iter().find(|n| n.id == id).unwrap().rect;
        // Fan out: one point leaves the focus, and every route's first segment is the same.
        let out: Vec<&EdgeRoute> = l
            .edges
            .iter()
            .filter(|r| d.edges[r.index].from == "focus")
            .collect();
        assert_eq!(out.len(), 5);
        let start = out[0].points[0];
        let trunk = out[0].points[1];
        for r in &out {
            assert_eq!(r.points[0], start, "one line out of the node");
            assert!(on_border(
                &rect(&d.edges[r.index].to),
                *r.points.last().unwrap()
            ));
            if r.points.len() > 2 {
                assert_eq!(r.points[1], trunk, "the trunk is shared");
            }
        }
        // Fan in: every route ends at the same point on the focus.
        let ins: Vec<&EdgeRoute> = l
            .edges
            .iter()
            .filter(|r| d.edges[r.index].to == "focus")
            .collect();
        let end = *ins[0].points.last().unwrap();
        assert!(
            ins.iter().all(|r| *r.points.last().unwrap() == end),
            "one line into the node"
        );
        // Nodes do not overlap; labels overlap no node, no other label and no other line.
        for (i, a) in l.nodes.iter().enumerate() {
            for b in &l.nodes[i + 1..] {
                assert!(!overlaps(&a.rect, &b.rect));
            }
        }
        let labelled: Vec<&EdgeRoute> = l.edges.iter().filter(|r| r.label_rect.is_some()).collect();
        assert_eq!(labelled.len(), 3);
        for r in &labelled {
            let lr = r.label_rect.unwrap();
            for n in &l.nodes {
                assert!(!overlaps(&lr, &n.rect), "{:?} label on {}", o, n.id);
            }
            for other in &l.edges {
                if other.index == r.index {
                    continue;
                }
                if let Some(or) = other.label_rect {
                    assert!(!overlaps(&lr, &or), "{o:?} labels overlap");
                }
                for w in other.points.windows(2) {
                    let sr = Rect {
                        x: w[0].0.min(w[1].0),
                        y: w[0].1.min(w[1].1),
                        w: (w[0].0 - w[1].0).abs().max(1),
                        h: (w[0].1 - w[1].1).abs().max(1),
                    };
                    assert!(
                        !overlaps(&lr, &sr),
                        "{o:?} label on another line {:?}",
                        other.points
                    );
                }
            }
        }
    }
}
