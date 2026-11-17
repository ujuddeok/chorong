(function () {
  'use strict';
  // The page draws a graph the Rust engine computed. Everything here is a view over that data:
  // filters, roll-ups and layout. Nothing is guessed; every line stands for index edges.
  if (typeof cytoscape === 'undefined' || typeof ELK === 'undefined') {
    const side = document.getElementById('side');
    if (side) side.textContent = 'The graph libraries did not load. This page uses the copy in Chorong\'s cache folder (graph-assets); it may have been moved or cleared. Build the graph again, or export it for sharing, which puts everything in the page.';
    return;
  }
  const D = JSON.parse(document.getElementById('cho-data').textContent);
  const byId = new Map(D.nodes.map(n => [n.id, n]));
  const kids = new Map();
  D.nodes.forEach(n => { if (n.parent) { if (!kids.has(n.parent)) kids.set(n.parent, []); kids.get(n.parent).push(n.id); } });
  const KINDS = D.view.edge_kinds;
  const KIND_ORDER = ['calls', 'constructs', 'renders', 'implements', 're-exports', 'type-uses', 'references'];
  const FIRST = 10, STEP = 10;      // disclosure: 10 shown, then 10 more, then a focused view
  const READABLE = 0.8;             // never open below this zoom (about 9 px text)
  const BUSY = 20;                  // more nodes than this: counts go on lines, not boxes
  const hidden = new Set();         // hidden edge kinds
  const expanded = new Set();
  const reveal = new Map();         // "side\0hub" -> how many are shown
  const focusStack = [];            // focused views: {hub, side, label}
  let showTests = D.view.kind === 'blast-radius';
  let showSide = false;
  let level = D.view.default_level;
  let cy = null, cur = null;
  const css = n => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
  const el = (t, props, ...ch) => { const e = document.createElement(t); Object.assign(e, props || {}); ch.forEach(c => e.append(c)); return e; };
  const FOCUS = D.nodes.filter(n => n.focus).map(n => n.id);

  function ancestors(id) { const a = []; let n = byId.get(id); while (n && n.parent) { a.push(n.parent); n = byId.get(n.parent); } return a; }
  function packageOf(id) { const a = ancestors(id); return a.length ? a[a.length - 1] : id; }
  function members(id) { return [id].concat((kids.get(id) || []).flatMap(members)); }
  function setLevel(l) {
    level = l; expanded.clear();
    if (l !== 'package') D.nodes.filter(n => n.kind === 'package').forEach(n => expanded.add(n.id));
    if (l === 'symbol') D.nodes.filter(n => n.kind === 'file').forEach(n => expanded.add(n.id));
    // Below package level the focus is always drawn; at package level its package is outlined.
    if (l !== 'package') FOCUS.forEach(f => ancestors(f).forEach(a => expanded.add(a)));
  }
  function rep(id) {
    const chain = [id].concat(ancestors(id));
    let i = chain.length - 1;
    while (i > 0 && expanded.has(chain[i])) i--;
    return chain[i];
  }
  // Lines that lead to the focus: one hop nearer it in the walk. Others are side lines.
  function onPath(e) {
    const a = byId.get(e.from).depth, b = byId.get(e.to).depth;
    if (a == null || b == null) return true;
    const dir = D.view.direction;
    if (dir === 'in') return b === a - 1;
    if (dir === 'out') return b === a + 1;
    return Math.abs(a - b) === 1;
  }

  // The model: which raw nodes and edges are in play after the filters.
  function model() {
    const alive = new Set(D.nodes.map(n => n.id));
    let testsHidden = 0;
    if (!showTests) D.nodes.forEach(n => { if (n.is_test && !n.focus) { alive.delete(n.id); if (n.kind === 'symbol') testsHidden++; } });
    D.nodes.forEach(n => { if (ancestors(n.id).some(a => !alive.has(a))) alive.delete(n.id); });
    const all = D.edges.filter(e => alive.has(e.from) && alive.has(e.to) && !hidden.has(e.kind));
    let sideLines = 0;
    const drawn = all.filter(e => { if (showSide || onPath(e)) return true; sideLines++; return false; });
    // Nodes that only side lines reached go with them.
    const before = new Set(all.flatMap(e => [e.from, e.to]));
    const after = new Set(drawn.flatMap(e => [e.from, e.to]));
    before.forEach(id => { if (!after.has(id) && !byId.get(id).focus) alive.delete(id); });
    pruneBoxes(alive);
    return { alive, all, drawn, testsHidden, sideLines };
  }
  function pruneBoxes(alive) {
    let changed = true;
    while (changed) {
      changed = false;
      for (const id of [...alive]) {
        const ks = kids.get(id) || [];
        if (ks.length && !ks.some(k => alive.has(k))) { alive.delete(id); changed = true; }
      }
    }
  }

  function aggregate(edges) {
    const m = new Map();
    edges.forEach(e => {
      const a = rep(e.from), b = rep(e.to);
      if (a === b) return;
      const key = a + '\u0000' + b;
      if (!m.has(key)) m.set(key, { from: a, to: b, kinds: {}, total: 0, mapped: 0, raw: [] });
      const x = m.get(key); x.kinds[e.kind] = (x.kinds[e.kind] || 0) + e.count; x.total += e.count; x.mapped += e.mapped || 0; x.raw.push(e);
    });
    const list = [...m.values()];
    const ids = [...new Set(list.flatMap(e => [e.from, e.to]))];
    const ix = new Map(ids.map((id, i) => [id, i]));
    const adj = ids.map(() => []);
    list.forEach(e => adj[ix.get(e.from)].push(ix.get(e.to)));
    const comp = scc(adj);
    list.forEach(e => {
      e.cycle = comp[ix.get(e.from)] === comp[ix.get(e.to)];
      const best = Object.entries(e.kinds).sort((p, q) => q[1] - p[1] || KIND_ORDER.indexOf(p[0]) - KIND_ORDER.indexOf(q[0]))[0];
      e.dom = best[0];
      e.typeOnly = Object.keys(e.kinds).every(k => k === 'type-uses');
      e.used = new Set(e.raw.map(r => r.to)).size;
    });
    return list;
  }
  function scc(adj) {
    const n = adj.length, index = Array(n).fill(-1), low = Array(n).fill(0), on = Array(n).fill(false), comp = Array(n).fill(-1);
    const st = []; let next = 0, nc = 0;
    for (let r = 0; r < n; r++) {
      if (index[r] !== -1) continue;
      const work = [[r, 0]];
      while (work.length) {
        const top = work[work.length - 1], v = top[0];
        if (top[1] === 0) { index[v] = low[v] = next++; st.push(v); on[v] = true; }
        if (top[1] < adj[v].length) {
          const u = adj[v][top[1]++];
          if (index[u] === -1) work.push([u, 0]); else if (on[u]) low[v] = Math.min(low[v], index[u]);
        } else {
          work.pop();
          if (work.length) { const p = work[work.length - 1][0]; low[p] = Math.min(low[p], low[v]); }
          if (low[v] === index[v]) { let u; do { u = st.pop(); on[u] = false; comp[u] = nc; } while (u !== v); nc++; }
        }
      }
    }
    return comp;
  }

  // What is drawn: representatives, roll-up lines, and "+N more" disclosure nodes.
  function scene() {
    const m = model();
    let { alive, drawn } = m;
    const top = focusStack[focusStack.length - 1];
    if (top) {
      // Focused view: the hub and everything on one side of it, no cap.
      const hubSet = new Set(members(top.hub));
      const keep = new Set(hubSet);
      drawn = drawn.filter(e => top.side === 'in' ? hubSet.has(e.to) && !hubSet.has(e.from) : hubSet.has(e.from) && !hubSet.has(e.to));
      drawn.forEach(e => { keep.add(e.from); keep.add(e.to); });
      alive = new Set([...alive].filter(id => keep.has(id) || members(id).some(x => keep.has(x))));
      // Many of them: group by file rather than shrink.
      const syms = new Set(drawn.map(e => top.side === 'in' ? e.from : e.to)).size;
      expanded.clear();
      D.nodes.filter(n => n.kind === 'package').forEach(n => expanded.add(n.id));
      if (syms <= 30) D.nodes.filter(n => n.kind === 'file').forEach(n => expanded.add(n.id));
      ancestors(top.hub).forEach(a => expanded.add(a));
      if (byId.get(top.hub).kind !== 'symbol') expanded.delete(top.hub);
    }
    let lines = aggregate(drawn);
    const more = [];
    if (!top) {
      const focusReps = new Set(FOCUS.map(rep));
      const degree = new Map();
      lines.forEach(e => { degree.set(e.from, (degree.get(e.from) || 0) + 1); degree.set(e.to, (degree.get(e.to) || 0) + 1); });
      const drop = new Set();
      ['in', 'out'].forEach(side => {
        const groups = new Map();
        lines.forEach(e => { const hub = side === 'in' ? e.to : e.from; if (!groups.has(hub)) groups.set(hub, []); groups.get(hub).push(e); });
        [...groups.keys()].sort().forEach(hub => {
          const list = groups.get(hub);
          const key = side + '\u0000' + hub;
          const shown = reveal.get(key) || FIRST;
          if (list.length <= shown) return;
          const other = e => side === 'in' ? e.from : e.to;
          list.sort((a, b) => b.total - a.total || byId.get(other(a)).label.localeCompare(byId.get(other(b)).label) || other(a).localeCompare(other(b)));
          // Only leaves fold, so nothing else loses its connection.
          const rest = list.slice(shown).filter(e => degree.get(other(e)) === 1 && !focusReps.has(other(e)));
          if (rest.length < 2) return;
          rest.forEach(e => drop.add(other(e)));
          more.push({ id: 'reveal:' + side + ':' + hub, hub, side, key, count: rest.length, total: rest.reduce((s, e) => s + e.total, 0), shown });
        });
      });
      lines = lines.filter(e => !drop.has(e.from) && !drop.has(e.to));
      more.forEach(x => lines.push(x.side === 'in'
        ? { from: x.id, to: x.hub, total: x.total, kinds: {}, raw: [], dom: 'references', reveal: 1 }
        : { from: x.hub, to: x.id, total: x.total, kinds: {}, raw: [], dom: 'references', reveal: 1 }));
      alive = new Set([...alive].filter(id => !drop.has(rep(id))));
    }
    const vis = new Set();
    [...alive].forEach(id => { const r = rep(id); vis.add(r); ancestors(r).forEach(a => vis.add(a)); });
    // An open box with nothing drawn inside it is not drawn.
    [...vis].forEach(id => { if (expanded.has(id) && (kids.get(id) || []).length && !(kids.get(id)).some(c => vis.has(c))) vis.delete(id); });
    lines = lines.filter(e => (vis.has(e.from) || e.reveal) && (vis.has(e.to) || e.reveal));
    lines.forEach((e, i) => { e.id = 'e' + i; });
    return { m, vis, lines, more, top };
  }

  function title(n) {
    if (n.kind === 'package' && !expanded.has(n.id)) { const c = (kids.get(n.id) || []).filter(x => byId.get(x).kind === 'file').length; return n.label + (c ? ' (' + c + (c === 1 ? ' file' : ' files') + ')' : ''); }
    if (n.kind === 'file' && !expanded.has(n.id)) { const c = (kids.get(n.id) || []).length; const base = n.label.split('/').pop(); return base + (c ? ' (' + c + ')' : ''); }
    if (n.kind === 'file') return n.label.split('/').pop();
    return n.label;
  }
  const width = label => Math.max(70, Math.min(300, label.length * 6.8 + 22));
  const isBox = id => { const n = byId.get(id); return n && n.kind !== 'symbol' && n.kind !== 'more' && !expanded.has(id); };
  const containsFocus = id => FOCUS.some(f => f === id || ancestors(f).includes(id));

  async function render() {
    cur = scene();
    const { vis, lines, more, top } = cur;
    const busy = vis.size > BUSY;
    // Sparse graphs carry "N used" in the box itself.
    const usedIn = new Map();
    if (!busy) lines.forEach(e => { if (isBox(e.to)) { if (!usedIn.has(e.to)) usedIn.set(e.to, new Set()); e.raw.forEach(r => usedIn.get(e.to).add(r.to)); } });
    const labelOf = id => {
      const x = more.find(m => m.id === id);
      if (x) return '+' + x.count + ' more';
      const n = byId.get(id); let t = title(n);
      if (usedIn.has(id) && usedIn.get(id).size > 1) t += ' · ' + usedIn.get(id).size + ' used';
      return t;
    };
    const sizes = new Map();
    function mk(id) {
      const label = labelOf(id);
      const o = { id };
      const ch = (kids.get(id) || []).filter(c => vis.has(c));
      if (expanded.has(id) && ch.length) {
        o.children = ch.map(mk);
        o.layoutOptions = { 'elk.padding': '[top=28,left=12,bottom=12,right=12]' };
      } else { o.width = width(label); o.height = 28; }
      sizes.set(id, label);
      return o;
    }
    const roots = [...vis].filter(id => { const p = byId.get(id).parent; return !p || !vis.has(p); }).sort().map(mk);
    more.forEach(x => { roots.push({ id: x.id, width: width('+' + x.count + ' more'), height: 28 }); sizes.set(x.id, '+' + x.count + ' more'); });
    const graph = {
      id: 'root',
      layoutOptions: {
        'elk.algorithm': 'layered', 'elk.direction': 'RIGHT', 'elk.hierarchyHandling': 'INCLUDE_CHILDREN',
        'elk.edgeRouting': 'ORTHOGONAL', 'elk.layered.mergeEdges': 'true',
        'elk.spacing.nodeNode': '16', 'elk.layered.spacing.nodeNodeBetweenLayers': '64', 'elk.spacing.edgeNode': '14',
        'elk.spacing.edgeEdge': '6', 'elk.layered.spacing.edgeNodeBetweenLayers': '14',
        'elk.layered.cycleBreaking.strategy': 'DEPTH_FIRST', 'elk.layered.crossingMinimization.strategy': 'LAYER_SWEEP',
        'elk.layered.nodePlacement.strategy': 'BRANDES_KOEPF', 'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
        'elk.separateConnectedComponents': 'false'
      },
      children: roots,
      edges: lines.map(e => ({ id: e.id, sources: [e.from], targets: [e.to] }))
    };
    let laid;
    try { laid = await new ELK().layout(graph); } catch (err) { document.getElementById('side').textContent = 'Layout failed: ' + err; return; }
    const abs = new Map();
    (function walk(n, ox, oy) { (n.children || []).forEach(c => { const x = ox + c.x, y = oy + c.y; abs.set(c.id, { x, y, w: c.width, h: c.height }); walk(c, x, y); }); })(laid, 0, 0);
    const routes = new Map();
    (function walkE(n) {
      (n.edges || []).forEach(le => {
        const off = le.container && le.container !== 'root' && abs.get(le.container) ? abs.get(le.container) : { x: 0, y: 0 };
        const s = (le.sections || [])[0];
        if (s) routes.set(le.id, [s.startPoint].concat(s.bendPoints || [], [s.endPoint]).map(p => ({ x: p.x + off.x, y: p.y + off.y })));
      });
      (n.children || []).forEach(walkE);
    })(laid);

    const els = [];
    vis.forEach(id => {
      const n = byId.get(id), a = abs.get(id); if (!a) return;
      const compound = expanded.has(id) && (kids.get(id) || []).some(c => vis.has(c));
      const fw = !compound && n.kind === 'package' && level === 'package' && containsFocus(id);
      els.push({ group: 'nodes', data: { id, parent: n.parent && vis.has(n.parent) ? n.parent : undefined, label: sizes.get(id), kind: n.kind, w: a.w, h: a.h },
        position: { x: a.x + a.w / 2, y: a.y + a.h / 2 },
        classes: [n.kind, n.focus || fw ? 'focus' : '', n.is_test ? 'test' : '', compound ? 'compound' : '', n.unresolved_in_repo ? 'unres' : ''].join(' ') });
    });
    more.forEach(x => { const a = abs.get(x.id); if (a) els.push({ group: 'nodes', data: { id: x.id, label: sizes.get(x.id), w: a.w, h: a.h }, position: { x: a.x + a.w / 2, y: a.y + a.h / 2 }, classes: 'more reveal' }); });
    lines.forEach(e => {
      const label = [busy && isBox(e.to) && e.used > 1 ? String(e.used) : '', e.mapped ? 'mapped' : ''].filter(Boolean).join(' ');
      const d = { id: e.id, source: e.from, target: e.to, kind: e.dom, total: e.total, label };
      const pts = routes.get(e.id), sa = abs.get(e.from), ta = abs.get(e.to);
      let cls = [e.typeOnly ? 'typeonly' : '', e.cycle ? 'cycle' : '', e.reveal ? 'reveal' : ''];
      if (pts && sa && ta) {
        // ELK's route, expressed the way Cytoscape draws segments: weights along and distances
        // across the line between the two node centres; endpoints relative to each centre.
        const S = { x: sa.x + sa.w / 2, y: sa.y + sa.h / 2 }, T = { x: ta.x + ta.w / 2, y: ta.y + ta.h / 2 };
        const dx = T.x - S.x, dy = T.y - S.y, L2 = dx * dx + dy * dy, L = Math.sqrt(L2);
        d.se = (pts[0].x - S.x) + 'px ' + (pts[0].y - S.y) + 'px';
        d.te = (pts[pts.length - 1].x - T.x) + 'px ' + (pts[pts.length - 1].y - T.y) + 'px';
        const bends = pts.slice(1, -1);
        if (bends.length && L > 0) {
          d.sw = bends.map(p => (((p.x - S.x) * dx + (p.y - S.y) * dy) / L2).toFixed(4)).join(' ');
          d.sd = bends.map(p => (((p.y - S.y) * dx - (p.x - S.x) * dy) / L).toFixed(2)).join(' ');
          cls.push('routed');
        } else cls.push('direct');
      }
      els.push({ group: 'edges', data: d, classes: cls.join(' ') });
    });
    if (cy) cy.destroy();
    const col = {}; KINDS.forEach(k => col[k] = css('--k-' + k));
    cy = cytoscape({
      container: document.getElementById('cy'), elements: els, layout: { name: 'preset' }, wheelSensitivity: 0.3, minZoom: 0.05, maxZoom: 3,
      // Nodes stay where ELK put them: the lines follow ELK's routes, so a dragged node would
      // leave its lines behind. Dragging anywhere pans; a click still selects.
      autoungrabify: true,
      style: [
        { selector: 'node', style: { 'label': 'data(label)', 'font-size': 11, 'color': css('--fg'), 'text-valign': 'center', 'text-halign': 'center', 'background-color': css('--sel'), 'border-width': 1, 'border-color': css('--border'), 'shape': 'round-rectangle', 'width': 'data(w)', 'height': 'data(h)', 'text-wrap': 'none' } },
        { selector: 'node.package', style: { 'background-color': css('--panel'), 'border-color': css('--muted'), 'font-weight': 600, 'shape': 'rectangle' } },
        { selector: 'node.file', style: { 'background-color': css('--bg'), 'shape': 'rectangle' } },
        { selector: 'node.compound', style: { 'text-valign': 'top', 'text-halign': 'center', 'text-margin-y': 14, 'background-opacity': 0.5, 'padding': 12, 'font-size': 11, 'color': css('--muted') } },
        { selector: 'node.external', style: { 'shape': 'cut-rectangle', 'background-color': css('--bg'), 'border-style': 'dashed', 'border-color': css('--muted') } },
        { selector: 'node.more', style: { 'shape': 'round-rectangle', 'border-style': 'dashed', 'background-color': css('--bg'), 'color': css('--muted') } },
        { selector: 'node.reveal', style: { 'color': css('--accent') } },
        { selector: 'node.test', style: { 'border-color': css('--k-implements'), 'border-width': 2 } },
        { selector: 'node.focus', style: { 'border-color': css('--accent'), 'border-width': 3, 'font-weight': 700 } },
        { selector: 'node:selected', style: { 'border-color': css('--accent'), 'border-width': 3 } },
        { selector: 'edge', style: { 'curve-style': 'straight', 'width': 'mapData(total, 1, 40, 1.2, 5)', 'line-color': ele => col[ele.data('kind')] || css('--muted'), 'target-arrow-color': ele => col[ele.data('kind')] || css('--muted'), 'target-arrow-shape': 'triangle', 'arrow-scale': 0.8, 'label': 'data(label)', 'font-size': 10, 'color': css('--muted'), 'text-background-color': css('--bg'), 'text-background-opacity': 0.9, 'text-background-padding': 2, 'opacity': 0.85 } },
        { selector: 'edge.routed', style: { 'curve-style': 'segments', 'edge-distances': 'node-position', 'segment-weights': ele => ele.data('sw'), 'segment-distances': ele => ele.data('sd'), 'source-endpoint': ele => ele.data('se'), 'target-endpoint': ele => ele.data('te') } },
        { selector: 'edge.direct', style: { 'source-endpoint': ele => ele.data('se'), 'target-endpoint': ele => ele.data('te') } },
        { selector: 'edge.typeonly', style: { 'line-style': 'dashed' } },
        { selector: 'edge.reveal', style: { 'line-style': 'dashed', 'line-color': css('--muted'), 'target-arrow-color': css('--muted') } },
        { selector: 'edge.cycle', style: { 'line-color': css('--cycle'), 'target-arrow-color': css('--cycle'), 'line-style': 'dotted', 'width': 2.4 } },
        { selector: '.dim', style: { 'opacity': 0.12 } },
        { selector: 'edge.hl', style: { 'opacity': 1, 'z-index': 99 } },
        { selector: 'node.hl', style: { 'opacity': 1 } }
      ]
    });
    window.__chorongCy = cy;
    cy.on('mouseover', 'node', ev => {
      const n = ev.target, nb = n.closedNeighborhood();
      cy.elements().addClass('dim'); nb.removeClass('dim'); nb.addClass('hl'); n.ancestors().removeClass('dim');
    });
    cy.on('mouseout', 'node', () => cy.elements().removeClass('dim hl'));
    cy.on('tap', 'node', ev => { const id = ev.target.id(); if (id.startsWith('reveal:')) disclose(more.find(x => x.id === id)); else show(id); });
    cy.on('tap', 'edge', ev => { const e = lines.find(x => x.id === ev.target.id()); if (e && !e.reveal) showLine(e); });
    cy.on('dbltap', 'node', ev => toggle(ev.target.id()));
    cy.on('tap', ev => { if (ev.target === cy) show(null); });
    openView(top);
    controls();
    minimap();
  }

  // Open fitted when the text stays readable; otherwise at a readable size on the focus.
  function openView(top) {
    cy.fit(undefined, 30);
    if (cy.zoom() > 1.25) { cy.zoom(1.25); cy.center(); }
    if (cy.zoom() >= READABLE) return;
    const target = top ? cy.getElementById(top.hub) : cy.nodes('.focus');
    cy.zoom(READABLE);
    if (target && target.length) cy.center(target); else cy.center();
  }

  function disclose(x) {
    if (!x) return;
    if (x.shown < FIRST + STEP) { reveal.set(x.key, x.shown + STEP); render(); return; }
    const hub = byId.get(x.hub);
    focusStack.push({ hub: x.hub, side: x.side, label: (x.side === 'in' ? 'Everything that uses ' : 'Everything used by ') + (hub ? title(hub) : x.hub) });
    render();
  }
  function back() { focusStack.pop(); setLevel(level); render(); }

  function toggle(id) {
    const n = byId.get(id);
    if (!n || !(kids.get(id) || []).length) return;
    if (expanded.has(id)) expanded.delete(id); else expanded.add(id);
    render();
  }

  // Side panel lists: one row per thing, its reference count and a quiet bar.
  function rows(list, max) {
    const ul = el('ul', { className: 'rows' });
    const top = Math.max(1, ...list.map(r => r.n));
    list.slice(0, max || 60).forEach(r => {
      const li = el('li', { title: r.title || '' });
      li.append(el('span', { className: 'nm', textContent: r.label }), el('span', { className: 'ct', textContent: String(r.n) }));
      const bar = el('span', { className: 'wt' }); bar.style.width = (100 * r.n / top).toFixed(1) + '%'; li.append(bar);
      ul.append(li);
    });
    if (list.length > (max || 60)) ul.append(el('li', { className: 'empty', textContent: '… ' + (list.length - (max || 60)) + ' more' }));
    return ul;
  }
  function tally(edges, end) {
    const m = new Map();
    edges.forEach(e => { const id = end === 'to' ? e.to : e.from; m.set(id, (m.get(id) || 0) + e.count); });
    return [...m.entries()].map(([id, n]) => ({ id, n, label: byId.get(id).label })).sort((a, b) => b.n - a.n || a.label.localeCompare(b.label));
  }
  function section(head, list) {
    cur.side.append(el('h3', { textContent: head + ' (' + list.length + ')' }));
    if (!list.length) { cur.side.append(el('div', { className: 'empty', textContent: 'none in this graph' })); return; }
    cur.side.append(rows(list));
  }
  function grouped(head, list) {
    const groups = new Map();
    list.forEach(r => { const p = packageOf(r.id); if (!groups.has(p)) groups.set(p, []); groups.get(p).push(r); });
    const gl = [...groups.entries()].map(([p, rs]) => ({ p, rs, n: rs.reduce((s, r) => s + r.n, 0) })).sort((a, b) => b.n - a.n || a.p.localeCompare(b.p));
    cur.side.append(el('h3', { textContent: head + ' (' + list.length + ')' }));
    if (!gl.length) { cur.side.append(el('div', { className: 'empty', textContent: 'none in this graph' })); return; }
    const top = Math.max(1, ...gl.map(g => g.n));
    gl.forEach(g => {
      const d = el('details', { open: gl.length <= 5 });
      const s = el('summary');
      s.append(el('span', { className: 'nm', textContent: (byId.get(g.p) || { label: g.p }).label + ' · ' + g.rs.length + (g.rs.length === 1 ? ' thing' : ' things') }), el('span', { className: 'ct', textContent: String(g.n) }));
      const bar = el('span', { className: 'wt' }); bar.style.width = (100 * g.n / top).toFixed(1) + '%'; s.append(bar);
      d.append(s, rows(g.rs));
      cur.side.append(d);
    });
  }

  function show(id) {
    const side = document.getElementById('side'); side.textContent = ''; cur.side = side;
    if (!id) { side.append(el('p', { className: 'empty', textContent: 'Click a node or a line for what it uses and what uses it. Double-click a package or file to open it. A "+N more" box shows 10 more, then all of them on their own.' })); return; }
    const n = byId.get(id);
    side.append(el('h2', { textContent: n.label }));
    const bits = [n.kind === 'symbol' ? (n.scip_kind || 'symbol') : n.kind];
    if (n.package) bits.push(n.package);
    if (n.is_test) bits.push('test (by path or name)');
    side.append(el('div', { className: 'k', textContent: bits.join(' · ') }));
    if (n.path) side.append(el('div', { className: 'k', textContent: n.path + (n.line ? ':' + n.line : '') }));
    if (n.kind === 'more') side.append(el('p', { textContent: n.hidden + ' declarations were dropped by the node cap when the graph was made. Export again with a larger --max-nodes, or from a node nearer the one you care about.' }));
    if (n.unresolved_in_repo) side.append(el('p', { textContent: 'References the index had no definition for in this repository package: unresolved, drawn to the package, never matched by name.' }));
    if (n.link) {
      side.append(el('h3', { textContent: 'Open' }));
      side.append(el('div', {}, el('a', { href: n.link, textContent: 'Open in Chorong' })));
      side.append(el('code', { textContent: n.open_cmd }));
      if (n.web_url) side.append(el('div', {}, el('a', { href: n.web_url, textContent: 'Open on the web', target: '_blank', rel: 'noopener' })));
    }
    const mem = new Set(members(id));
    const inE = cur.m.all.filter(e => mem.has(e.to) && !mem.has(e.from));
    const outE = cur.m.all.filter(e => mem.has(e.from) && !mem.has(e.to));
    if (n.kind === 'package' || n.kind === 'file') {
      grouped('Used by', tally(inE, 'from'));
      grouped('Uses', tally(outE, 'to'));
    } else {
      section('Used by', tally(inE, 'from'));
      section('Uses', tally(outE, 'to'));
    }
    if (n.unresolved_calls && n.unresolved_calls.length) {
      side.append(el('h3', { textContent: 'Calls the index did not resolve' }));
      const ul = el('ul'); n.unresolved_calls.forEach(c => ul.append(el('li', { textContent: c.name + ' (line ' + c.line + ')' }))); side.append(ul);
      side.append(el('div', { className: 'k', textContent: 'Seen by Tree-sitter only; not matched by name, so no edge.' }));
    }
    if ((kids.get(id) || []).length) side.append(el('p', {}, el('button', { textContent: expanded.has(id) ? 'Close' : 'Open', onclick: () => toggle(id) })));
  }
  function showLine(e) {
    const side = document.getElementById('side'); side.textContent = ''; cur.side = side;
    const a = byId.get(e.from), b = byId.get(e.to);
    side.append(el('h2', { textContent: title(a) + ' → ' + title(b) }));
    const verified = e.total - (e.mapped || 0);
    if (verified) side.append(el('div', { className: 'k', textContent: verified + ' references (verified by the index)' }));
    if (e.mapped) side.append(el('div', { className: 'k', textContent: e.mapped + ' references through a built declaration (dist) mapped to this source: mapped (unverified)' }));
    side.append(el('div', { className: 'k', textContent: Object.entries(e.kinds).map(([k, c]) => k + ' ' + c).join(' · ') }));
    if (e.cycle) side.append(el('p', { textContent: 'In a cycle: the two sides reach each other.' }));
    section('Uses', tally(e.raw, 'to'));
    section('From', tally(e.raw, 'from'));
  }

  function controls() {
    const bar = document.getElementById('bar'); bar.textContent = '';
    const top = focusStack[focusStack.length - 1];
    if (top) {
      bar.append(el('div', { className: 'grp' }, el('button', { textContent: '← Back', onclick: back }), el('b', { textContent: top.label })));
    }
    const g1 = el('div', { className: 'grp' }, el('span', { className: 'k', textContent: 'Level' }));
    ['package', 'file', 'symbol'].forEach(l => g1.append(el('button', { textContent: l, className: level === l && !top ? 'on' : '', onclick: () => { focusStack.length = 0; setLevel(l); render(); } })));
    bar.append(g1);
    const g0 = el('div', { className: 'grp' });
    const tests = D.nodes.filter(n => n.is_test && n.kind === 'symbol').length;
    if (tests) {
      const cb = el('input', { type: 'checkbox', checked: showTests, onchange: () => { showTests = cb.checked; render(); } });
      g0.append(el('label', { className: 'kind', title: 'Test files are marked by path and name' }, cb, showTests ? 'tests' : 'tests (' + cur.m.testsHidden + ' hidden)'));
    }
    const sideCount = showSide ? D.edges.filter(e => !onPath(e)).length : cur.m.sideLines;
    if (sideCount) {
      const cb = el('input', { type: 'checkbox', checked: showSide, onchange: () => { showSide = cb.checked; render(); } });
      g0.append(el('label', { className: 'kind', title: 'Lines between things in the picture that do not lead to the focus' }, cb, 'other lines (' + sideCount + ')'));
    }
    if (g0.childNodes.length) bar.append(g0);
    const counts = {}; D.edges.forEach(e => counts[e.kind] = (counts[e.kind] || 0) + e.count);
    const g2 = el('div', { className: 'grp' });
    KINDS.filter(k => counts[k]).forEach(k => {
      const cb = el('input', { type: 'checkbox', checked: !hidden.has(k), onchange: () => { if (cb.checked) hidden.delete(k); else hidden.add(k); render(); } });
      const sw = el('span', { className: 'sw' + (k === 'type-uses' ? ' dash' : '') }); sw.style.setProperty('--c', 'var(--k-' + k + ')');
      g2.append(el('label', { className: 'kind', title: 'Show ' + k + ' edges' }, cb, sw, k + ' ' + counts[k]));
    });
    bar.append(g2);
    const g3 = el('div', { className: 'grp' });
    g3.append(el('button', { textContent: 'Fit', title: 'Show the whole graph, however small', onclick: () => cy.fit(undefined, 30) }));
    g3.append(el('button', { textContent: 'Theme: ' + (document.documentElement.dataset.theme || 'auto'), onclick: () => {
      const c = document.documentElement.dataset.theme; const nxt = c === 'dark' ? 'light' : c === 'light' ? '' : 'dark';
      if (nxt) document.documentElement.dataset.theme = nxt; else delete document.documentElement.dataset.theme;
      render();
    } }));
    bar.append(g3);
  }

  // Overview map: shown while the graph does not fit; click or drag it to move.
  function minimap() {
    let c = document.getElementById('mini');
    if (!c) {
      c = el('canvas', { id: 'mini', width: 200, height: 130 });
      document.getElementById('cy').append(c);
      const go = ev => { if (!cy || !c.__t) return; const r = c.getBoundingClientRect(), t = c.__t; const x = (ev.clientX - r.left - t.ox) / t.s + t.bx, y = (ev.clientY - r.top - t.oy) / t.s + t.by; const z = cy.zoom(); cy.pan({ x: cy.width() / 2 - x * z, y: cy.height() / 2 - y * z }); };
      let drag = false;
      c.addEventListener('mousedown', ev => { drag = true; go(ev); ev.preventDefault(); });
      window.addEventListener('mousemove', ev => { if (drag) go(ev); });
      window.addEventListener('mouseup', () => { drag = false; });
    }
    const draw = () => {
      const bb = cy.elements().boundingBox(), ext = cy.extent();
      const fits = ext.x1 <= bb.x1 && ext.y1 <= bb.y1 && ext.x2 >= bb.x2 && ext.y2 >= bb.y2;
      c.style.display = fits || !cy.nodes().length ? 'none' : 'block';
      if (fits) return;
      const ctx = c.getContext('2d'), W = c.width, H = c.height;
      const s = Math.min((W - 8) / bb.w, (H - 8) / bb.h), ox = (W - bb.w * s) / 2, oy = (H - bb.h * s) / 2;
      c.__t = { s, ox, oy, bx: bb.x1, by: bb.y1 };
      ctx.clearRect(0, 0, W, H);
      ctx.fillStyle = css('--panel'); ctx.fillRect(0, 0, W, H);
      cy.nodes().forEach(n => {
        if (n.isParent()) return;
        const b = n.boundingBox();
        ctx.fillStyle = n.hasClass('focus') ? css('--accent') : css('--muted');
        ctx.fillRect(ox + (b.x1 - bb.x1) * s, oy + (b.y1 - bb.y1) * s, Math.max(1.5, b.w * s), Math.max(1.5, b.h * s));
      });
      ctx.strokeStyle = css('--accent'); ctx.lineWidth = 1.5;
      ctx.strokeRect(ox + (ext.x1 - bb.x1) * s, oy + (ext.y1 - bb.y1) * s, ext.w * s, ext.h * s);
    };
    cy.on('viewport resize', draw);
    draw();
  }

  setLevel(level);
  cur = { m: model() };
  show(null);
  window.addEventListener('resize', () => cy && cy.resize());
  render().then(() => { window.__chorongGraphReady = true; });
})();
