//! A collection's flow drawn as SVG: drag the background to pan, the wheel
//! to zoom, a node to move it, from a port (right edge) onto a node to wire
//! them. Click a node or an edge to pick it; Delete removes what is picked.
//! Loop edges are dashed; nodes the start cannot reach are dimmed; a run's
//! node states colour the nodes and the ports they fired.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;
use herdr_mcp_api::flow::START;
use herdr_mcp_api::playground::{NODE_H, NODE_W, layout, tool_label};
use herdr_mcp_api::{Edge, Flow, Node, NodeKind, NodeState, Point, Port, Uuid};

#[derive(Clone, Copy, PartialEq)]
struct View {
    x: f64,
    y: f64,
    zoom: f64,
}

const HOME: View = View {
    x: 24.0,
    y: 24.0,
    zoom: 1.0,
};

#[derive(Clone, PartialEq)]
enum Drag {
    Pan {
        from: (f64, f64),
        view: View,
    },
    Node {
        id: String,
        from: (f64, f64),
        at: Point,
        moved: bool,
    },
    Wire {
        from: String,
        port: Port,
        start: Point,
        client: (f64, f64),
        end: Point,
    },
}

/// An edit the canvas asks for; the collection applies and saves it.
#[derive(Clone, Debug, PartialEq)]
pub enum CanvasEdit {
    Move(String, Point),
    Connect(String, Port, String),
    Disconnect(Edge),
    Remove(String),
}

fn client(e: &MouseEvent) -> (f64, f64) {
    let p = e.client_coordinates();
    (p.x, p.y)
}

fn height(node: &Node) -> f32 {
    NODE_H.max(node.kind.ports().len() as f32 * 18.0 + 12.0)
}

/// Where port `i` of `n` sits on a node at `at`.
fn port_at(node: &Node, at: Point, i: usize, n: usize) -> Point {
    Point {
        x: at.x + NODE_W,
        y: at.y + height(node) * (i + 1) as f32 / (n + 1) as f32,
    }
}

fn input_at(node: &Node, at: Point) -> Point {
    Point {
        x: at.x,
        y: at.y + height(node) / 2.0,
    }
}

fn curve(a: Point, b: Point, back: bool) -> String {
    if back {
        let drop = 70.0 + (a.x - b.x).abs() * 0.15;
        format!(
            "M{} {} C{} {} {} {} {} {}",
            a.x,
            a.y,
            a.x + 60.0,
            a.y + drop,
            b.x - 60.0,
            b.y + drop,
            b.x,
            b.y
        )
    } else {
        let dx = ((b.x - a.x).abs() / 2.0).max(40.0);
        format!(
            "M{} {} C{} {} {} {} {} {}",
            a.x,
            a.y,
            a.x + dx,
            a.y,
            b.x - dx,
            b.y,
            b.x,
            b.y
        )
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// What a node does, under its id.
fn detail(node: &Node, names: &HashMap<Uuid, String>) -> String {
    match &node.kind {
        NodeKind::Start => "start".into(),
        NodeKind::Tool { .. } => node.step().map(|s| tool_label(&s)).unwrap_or_default(),
        NodeKind::Branch { rules } => format!("branch · {} rule(s)", rules.len()),
        NodeKind::Run { collection, .. } => match names.get(collection) {
            Some(n) => format!("run {n}"),
            None => "run: missing collection".into(),
        },
        NodeKind::Wait { ms } => format!("wait {ms} ms"),
        NodeKind::Fail { message } if message.is_empty() => "fail".into(),
        NodeKind::Fail { message } => format!("fail: {message}"),
    }
}

fn state_class(s: Option<&NodeState>) -> &'static str {
    match s {
        None => "",
        Some(NodeState::Running) => " node-running",
        Some(NodeState::Done(_)) => " node-done",
        Some(NodeState::Failed) => " node-failed",
        Some(NodeState::Skipped) => " node-skipped",
        Some(NodeState::Cancelled) => " node-cancelled",
    }
}

#[component]
pub fn Canvas(
    flow: Flow,
    /// Other collections' names, for run nodes.
    collections: Vec<(Uuid, String)>,
    /// The last run's node states; empty when there is none.
    states: HashMap<String, NodeState>,
    selected: Signal<Option<String>>,
    on_edit: EventHandler<CanvasEdit>,
) -> Element {
    let mut view = use_signal(|| HOME);
    let mut drag: Signal<Option<Drag>> = use_signal(|| None);
    let mut picked_edge: Signal<Option<Edge>> = use_signal(|| None);

    let mut at = layout(&flow);
    if let Some(Drag::Node { id, at: p, .. }) = &*drag.read() {
        at.insert(id.clone(), *p);
    }
    let shape = flow.shape();
    let reachable: HashSet<String> = flow.reachable().into_iter().map(str::to_string).collect();
    let names: HashMap<Uuid, String> = collections.into_iter().collect();
    let v = view();

    let edges: Vec<(Edge, String, bool, bool)> = flow
        .edges
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let (from, to) = (flow.node(&e.from)?, flow.node(&e.to)?);
            let ports = from.kind.ports();
            let k = ports.iter().position(|p| *p == e.port)?;
            let a = port_at(from, at[&from.id], k, ports.len());
            let b = input_at(to, at[&to.id]);
            let back = shape.back.contains(&i);
            let lit = states.get(&e.from) == Some(&NodeState::Done(e.port));
            Some((e.clone(), curve(a, b, back), back, lit))
        })
        .collect();
    let wire = match &*drag.read() {
        Some(Drag::Wire { start, end, .. }) => Some(curve(*start, *end, false)),
        _ => None,
    };

    let on_move = move |e: MouseEvent| {
        let (cx, cy) = client(&e);
        let zoom = view.peek().zoom;
        let mut d = drag.write();
        match d.as_mut() {
            Some(Drag::Pan { from, view: start }) => {
                view.set(View {
                    x: start.x + cx - from.0,
                    y: start.y + cy - from.1,
                    ..*start
                });
            }
            Some(Drag::Node {
                from, at, moved, ..
            }) => {
                let (dx, dy) = ((cx - from.0) / zoom, (cy - from.1) / zoom);
                if dx.abs() + dy.abs() > 0.0 {
                    *at = Point {
                        x: at.x + dx as f32,
                        y: at.y + dy as f32,
                    };
                    *from = (cx, cy);
                    *moved = true;
                }
            }
            Some(Drag::Wire {
                start, client, end, ..
            }) => {
                *end = Point {
                    x: start.x + ((cx - client.0) / zoom) as f32,
                    y: start.y + ((cy - client.1) / zoom) as f32,
                };
            }
            None => {}
        }
    };
    let on_up = move |_| {
        if let Some(Drag::Node {
            id,
            at,
            moved: true,
            ..
        }) = drag.take()
        {
            on_edit.call(CanvasEdit::Move(id, at));
        }
    };
    let on_key = move |e: KeyboardEvent| {
        if !matches!(e.key(), Key::Delete | Key::Backspace) {
            return;
        }
        if let Some(edge) = picked_edge.take() {
            on_edit.call(CanvasEdit::Disconnect(edge));
        } else if let Some(id) = selected().filter(|id| id != START) {
            selected.set(None);
            on_edit.call(CanvasEdit::Remove(id));
        }
    };
    let mut zoom_by = move |f: f64| {
        let v = *view.peek();
        view.set(View {
            zoom: (v.zoom * f).clamp(0.3, 2.5),
            ..v
        });
    };

    rsx! {
        div { class: "canvas-wrap",
            div { class: "canvas-tools",
                button { class: "link-btn", title: "Zoom in", onclick: move |_| zoom_by(1.2), "+" }
                button { class: "link-btn", title: "Zoom out", onclick: move |_| zoom_by(1.0 / 1.2), "−" }
                button { class: "link-btn", title: "Back to the start", onclick: move |_| view.set(HOME), "⌂" }
                if let Some(edge) = picked_edge() {
                    button {
                        class: "link-btn link-danger",
                        onclick: move |_| {
                            picked_edge.set(None);
                            on_edit.call(CanvasEdit::Disconnect(edge.clone()));
                        },
                        "✕ edge"
                    }
                }
            }
            svg {
                class: "canvas",
                tabindex: "0",
                onmousedown: move |e| {
                    selected.set(None);
                    picked_edge.set(None);
                    drag.set(Some(Drag::Pan { from: client(&e), view: *view.peek() }));
                },
                onmousemove: on_move,
                onmouseup: on_up,
                onmouseleave: on_up,
                onwheel: move |e: WheelEvent| {
                    e.prevent_default();
                    zoom_by(if e.delta().strip_units().y > 0.0 { 1.0 / 1.1 } else { 1.1 });
                },
                onkeydown: on_key,
                g { transform: "translate({v.x} {v.y}) scale({v.zoom})",
                    for (edge, d, back, lit) in edges {
                        {
                            let picked = picked_edge.read().as_ref() == Some(&edge);
                            let class = format!(
                                "edge{}{}{}{}",
                                if back { " edge-loop" } else { "" },
                                if edge.port == Port::Error { " edge-error" } else { "" },
                                if lit { " edge-lit" } else { "" },
                                if picked { " edge-picked" } else { "" },
                            );
                            let key = format!("{}.{}>{}", edge.from, edge.port, edge.to);
                            rsx! {
                                g { key: "{key}",
                                    path { class: "{class}", d: "{d}" }
                                    path {
                                        class: "edge-hit",
                                        d: "{d}",
                                        onmousedown: move |e| {
                                            e.stop_propagation();
                                            picked_edge.set(Some(edge.clone()));
                                        },
                                    }
                                }
                            }
                        }
                    }
                    if let Some(d) = wire {
                        path { class: "edge edge-wire", d: "{d}" }
                    }
                    for node in flow.nodes.iter().cloned() {
                        {
                            let p = at[&node.id];
                            let h = height(&node);
                            let ports = node.kind.ports();
                            let n = ports.len();
                            let id = node.id.clone();
                            let class = format!(
                                "node node-{}{}{}{}",
                                node.kind.name(),
                                state_class(states.get(&id)),
                                if reachable.contains(&id) { "" } else { " node-dim" },
                                if selected.read().as_deref() == Some(id.as_str()) { " node-sel" } else { "" },
                            );
                            let title = clip(&id, 22);
                            let sub = clip(&detail(&node, &names), 26);
                            let input = input_at(&node, p);
                            let (grab, target) = (id.clone(), id.clone());
                            rsx! {
                                g { key: "{id}",
                                    g {
                                        class: "{class}",
                                        onmousedown: move |e| {
                                            e.stop_propagation();
                                            selected.set(Some(grab.clone()));
                                            picked_edge.set(None);
                                            drag.set(Some(Drag::Node { id: grab.clone(), from: client(&e), at: p, moved: false }));
                                        },
                                        onmouseup: move |_| {
                                            if let Some(Drag::Wire { from, port, .. }) = drag.peek().clone()
                                                && from != target
                                            {
                                                on_edit.call(CanvasEdit::Connect(from, port, target.clone()));
                                            }
                                        },
                                        rect { x: "{p.x}", y: "{p.y}", width: "{NODE_W}", height: "{h}", rx: "8" }
                                        text { class: "node-title", x: "{p.x + 12.0}", y: "{p.y + 22.0}", "{title}" }
                                        text { class: "node-sub", x: "{p.x + 12.0}", y: "{p.y + 40.0}", "{sub}" }
                                    }
                                    if !matches!(node.kind, NodeKind::Start) {
                                        circle { class: "port port-in", cx: "{input.x}", cy: "{input.y}", r: "5" }
                                    }
                                    for (k, port) in ports.into_iter().enumerate() {
                                        {
                                            let q = port_at(&node, p, k, n);
                                            let from = id.clone();
                                            rsx! {
                                                g { key: "{port}",
                                                    if !port.is_out() {
                                                        text {
                                                            class: "port-label",
                                                            x: "{q.x - 9.0}",
                                                            y: "{q.y + 4.0}",
                                                            text_anchor: "end",
                                                            "{port}"
                                                        }
                                                    }
                                                    circle {
                                                        class: if port == Port::Error { "port port-out port-error" } else { "port port-out" },
                                                        cx: "{q.x}",
                                                        cy: "{q.y}",
                                                        r: "6",
                                                        onmousedown: move |e| {
                                                            e.stop_propagation();
                                                            drag.set(Some(Drag::Wire { from: from.clone(), port, start: q, client: client(&e), end: q }));
                                                        },
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
