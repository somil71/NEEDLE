import * as vscode from 'vscode';

export class NeedleGraphPanel {
    public static currentPanel: NeedleGraphPanel | undefined;
    private static readonly viewType = 'needle.graph';

    private readonly _panel: vscode.WebviewPanel;
    private _disposables: vscode.Disposable[] = [];

    public static createOrShow(extensionUri: vscode.Uri, serverUrl: string) {
        const col = vscode.window.activeTextEditor?.viewColumn ?? vscode.ViewColumn.One;

        if (NeedleGraphPanel.currentPanel) {
            NeedleGraphPanel.currentPanel._panel.reveal(col);
            return;
        }
        const panel = vscode.window.createWebviewPanel(
            NeedleGraphPanel.viewType,
            'Needle — Knowledge Graph',
            col,
            {
                enableScripts: true,
                retainContextWhenHidden: true,
                localResourceRoots: [vscode.Uri.joinPath(extensionUri, 'media')],
            }
        );
        NeedleGraphPanel.currentPanel = new NeedleGraphPanel(panel, extensionUri, serverUrl);
    }

    private constructor(
        panel: vscode.WebviewPanel,
        private readonly extensionUri: vscode.Uri,
        private readonly serverUrl: string,
    ) {
        this._panel = panel;
        this._panel.webview.html = this.buildHtml();

        this._panel.onDidDispose(() => this.dispose(), null, this._disposables);

        this._panel.webview.onDidReceiveMessage(
            async (msg: { type: string; path?: string; line?: number }) => {
                if (msg.type === 'openFile' && msg.path !== undefined && msg.line !== undefined) {
                    vscode.commands.executeCommand('needle.openFile', msg.path, msg.line);
                }
                if (msg.type === 'openBrowser') {
                    vscode.env.openExternal(vscode.Uri.parse(`${this.serverUrl}/#/graph`));
                }
                // Proxy graph data fetch through extension process (webview can't reach localhost)
                if (msg.type === 'loadGraph') {
                    try {
                        const [gRes, sRes] = await Promise.all([
                            fetch(`${this.serverUrl}/api/graph`, { signal: AbortSignal.timeout(15000) }),
                            fetch(`${this.serverUrl}/api/status`, { signal: AbortSignal.timeout(5000) }),
                        ]);
                        if (!gRes.ok) throw new Error(`Server returned ${gRes.status}`);
                        const graphData = await gRes.json();
                        const statusData = sRes.ok ? await sRes.json() : {};
                        this._panel.webview.postMessage({ type: 'graphData', graphData, statusData });
                    } catch (e: unknown) {
                        const msg2 = e instanceof Error ? e.message : 'Failed to load graph';
                        this._panel.webview.postMessage({ type: 'graphError', message: msg2 });
                    }
                }
            },
            null,
            this._disposables
        );
    }

    private dispose() {
        NeedleGraphPanel.currentPanel = undefined;
        this._panel.dispose();
        while (this._disposables.length) {
            this._disposables.pop()?.dispose();
        }
    }

    private buildHtml(): string {
        const nonce = Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
        const url = this.serverUrl;
        const d3Uri = this._panel.webview.asWebviewUri(
            vscode.Uri.joinPath(this.extensionUri, 'media', 'd3.min.js')
        );

        return /* html */`<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy"
  content="default-src 'none';
    script-src ${d3Uri} 'nonce-${nonce}';
    style-src 'unsafe-inline';">
<title>Needle — Knowledge Graph</title>
<style>
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

  body {
    background: var(--vscode-editor-background, #0c0c0d);
    color: var(--vscode-foreground);
    font-family: var(--vscode-font-family);
    font-size: var(--vscode-font-size);
    height: 100vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* ── Top toolbar ── */
  .toolbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--vscode-panel-border, #333);
    flex-shrink: 0;
    flex-wrap: wrap;
  }
  .toolbar h2 { font-size: 13px; font-weight: 600; margin-right: 4px; }
  .stat { font-size: 11px; color: var(--vscode-descriptionForeground); }
  .spacer { flex: 1; }
  .filter-wrap { display: flex; gap: 6px; flex-wrap: wrap; }
  label { font-size: 11px; display: flex; align-items: center; gap: 3px; cursor: pointer; }
  input[type=checkbox] { accent-color: #7C3AED; }
  .search-input {
    background: var(--vscode-input-background);
    color: var(--vscode-input-foreground);
    border: 1px solid var(--vscode-input-border, transparent);
    border-radius: 4px;
    padding: 3px 7px;
    font: inherit;
    font-size: 12px;
    outline: none;
    width: 180px;
  }
  .search-input:focus { border-color: var(--vscode-focusBorder); }
  .btn {
    background: var(--vscode-button-secondaryBackground, #2d2d30);
    color: var(--vscode-button-secondaryForeground, #ccc);
    border: 1px solid var(--vscode-button-border, transparent);
    border-radius: 4px;
    padding: 4px 10px;
    font: inherit;
    font-size: 11px;
    cursor: pointer;
  }
  .btn:hover { background: var(--vscode-button-secondaryHoverBackground, #3e3e42); }

  /* ── Main area ── */
  .main { display: flex; flex: 1; overflow: hidden; }

  /* ── D3 canvas ── */
  #canvas { flex: 1; overflow: hidden; position: relative; }
  #canvas svg { width: 100%; height: 100%; }

  /* ── Detail panel ── */
  #detail {
    width: 240px;
    flex-shrink: 0;
    border-left: 1px solid var(--vscode-panel-border, #333);
    overflow-y: auto;
    padding: 12px;
    font-size: 12px;
    display: none;
  }
  #detail.open { display: block; }
  #detail h3 { font-size: 13px; margin-bottom: 6px; word-break: break-all; }
  #detail .kind-badge {
    display: inline-block;
    padding: 1px 6px;
    border-radius: 10px;
    font-size: 10px;
    margin-bottom: 8px;
  }
  #detail .field { margin-bottom: 6px; }
  #detail .field-label { color: var(--vscode-descriptionForeground); font-size: 10px; text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 2px; }
  #detail .field-val { word-break: break-all; }
  #detail .field-val a { color: var(--vscode-textLink-foreground); cursor: pointer; text-decoration: none; }
  #detail .field-val a:hover { text-decoration: underline; }
  #detail ul { padding-left: 16px; }
  #detail ul li { margin-bottom: 2px; }
  #detail ul li a { color: var(--vscode-textLink-foreground); cursor: pointer; }

  /* ── Loading / error ── */
  .overlay {
    position: absolute; inset: 0;
    display: flex; align-items: center; justify-content: center;
    flex-direction: column; gap: 12px;
    background: var(--vscode-editor-background, #0c0c0d);
    font-size: 13px;
    color: var(--vscode-descriptionForeground);
  }
  .overlay.hidden { display: none; }

  /* ── Node colors ── */
  .node-module   { fill: #71717a; }
  .node-function { fill: #6366f1; }
  .node-method   { fill: #3b82f6; }
  .node-class    { fill: #8b5cf6; }
  .node-struct   { fill: #f97316; }
  .node-trait    { fill: #f59e0b; }
  .node-endpoint { fill: #10b981; }
</style>
</head>
<body>

<!-- Toolbar -->
<div class="toolbar">
  <h2>Knowledge Graph</h2>
  <span id="stats" class="stat">Loading…</span>
  <div class="spacer"></div>
  <input id="search" class="search-input" type="search" placeholder="Filter nodes…">
  <div class="filter-wrap" id="filters"></div>
  <button class="btn" id="fitBtn">Fit</button>
  <button class="btn" id="browserBtn">Open in browser ↗</button>
</div>

<!-- Main -->
<div class="main">
  <div id="canvas">
    <div class="overlay" id="overlay">
      <div id="overlayMsg">Loading graph…</div>
    </div>
    <svg id="svg"></svg>
  </div>
  <div id="detail"></div>
</div>

<script nonce="${nonce}" src="${d3Uri}"></script>
<script nonce="${nonce}">
const KIND_COLORS = {
  module: '#71717a', function: '#6366f1', method: '#3b82f6',
  class: '#8b5cf6', struct: '#f97316', trait: '#f59e0b', endpoint: '#10b981',
};
const KIND_ORDER = ['endpoint','class','struct','trait','function','method','module'];

let allNodes = [], allEdges = [], simulation, svg, g, linkSel, nodeSel, labelSel;
let activeFilters = new Set(['function','method','class','struct','trait','endpoint','module']);
let searchTerm = '';
let selectedNode = null;

const vscode = acquireVsCodeApi();

// ── Fetch graph data via extension process (avoids localhost CSP) ─────────────
function loadGraph() {
  vscode.postMessage({ type: 'loadGraph' });
}

window.addEventListener('message', e => {
  const msg = e.data;
  if (msg.type === 'graphData') {
    const { nodes, edges, stats } = msg.graphData;
    const status = msg.statusData || {};
    allNodes = nodes;
    allEdges = edges;
    document.getElementById('stats').textContent =
      stats.total_nodes + ' nodes · ' + stats.total_edges + ' edges · ' +
      (status.total_files || 0) + ' files';
    buildFilters(stats);
    document.getElementById('overlay').classList.add('hidden');
    renderGraph();
  }
  if (msg.type === 'graphError') {
    document.getElementById('overlayMsg').textContent = '⚠ Could not load graph: ' + msg.message +
      '. Make sure needle serve is running.';
  }
});

// ── Filters ───────────────────────────────────────────────────────────────────
function buildFilters(stats) {
  const wrap = document.getElementById('filters');
  const kinds = [
    ['endpoint', 'Endpoints (' + stats.endpoints + ')'],
    ['class',    'Classes (' + stats.classes + ')'],
    ['function', 'Functions (' + stats.functions + ')'],
    ['method',   'Methods (' + stats.methods + ')'],
    ['module',   'Modules (' + stats.modules + ')'],
  ];
  wrap.innerHTML = kinds.map(([k, lbl]) =>
    '<label><input type="checkbox" value="' + k + '" ' + (activeFilters.has(k) ? 'checked' : '') + '>' + lbl + '</label>'
  ).join('');
  wrap.querySelectorAll('input').forEach(cb => {
    cb.addEventListener('change', () => {
      if (cb.checked) activeFilters.add(cb.value); else activeFilters.delete(cb.value);
      rerenderGraph();
    });
  });
}

// ── Render ────────────────────────────────────────────────────────────────────
function renderGraph() {
  const svgEl = document.getElementById('svg');
  const W = svgEl.clientWidth || 800;
  const H = svgEl.clientHeight || 600;

  svg = d3.select('#svg');
  svg.selectAll('*').remove();

  // Arrow marker
  svg.append('defs').append('marker')
    .attr('id', 'arrow')
    .attr('viewBox', '0 -4 8 8')
    .attr('refX', 14).attr('refY', 0)
    .attr('markerWidth', 6).attr('markerHeight', 6)
    .attr('orient', 'auto')
    .append('path').attr('d', 'M0,-4L8,0L0,4').attr('fill', '#555');

  g = svg.append('g');

  svg.call(
    d3.zoom().scaleExtent([0.05, 4]).on('zoom', e => g.attr('transform', e.transform))
  );

  rerenderGraph();
}

function rerenderGraph() {
  if (!g) return;
  const term = searchTerm.toLowerCase();

  const visNodes = allNodes.filter(n =>
    activeFilters.has(n.kind) &&
    (!term || n.name.toLowerCase().includes(term) || n.file_path.toLowerCase().includes(term))
  );
  const visIds = new Set(visNodes.map(n => n.id));
  const visEdges = allEdges.filter(e =>
    e.kind !== 'contains' && visIds.has(e.from) && visIds.has(e.to)
  );

  // Degree map for sizing
  const degree = {};
  visEdges.forEach(e => {
    degree[e.from] = (degree[e.from] || 0) + 1;
    degree[e.to]   = (degree[e.to] || 0) + 1;
  });

  const svgEl = document.getElementById('svg');
  const W = svgEl.clientWidth || 900;
  const H = svgEl.clientHeight || 600;

  if (simulation) simulation.stop();

  simulation = d3.forceSimulation(visNodes)
    .force('link', d3.forceLink(visEdges).id(d => d.id).distance(60).strength(0.5))
    .force('charge', d3.forceManyBody().strength(-120))
    .force('center', d3.forceCenter(W / 2, H / 2))
    .force('collision', d3.forceCollide().radius(d => nodeR(d, degree) + 4));

  g.selectAll('*').remove();

  linkSel = g.append('g').attr('opacity', 0.4)
    .selectAll('line').data(visEdges).join('line')
    .attr('stroke', d => d.kind === 'imports' ? '#6366f1' : '#555')
    .attr('stroke-width', 1)
    .attr('marker-end', 'url(#arrow)');

  nodeSel = g.append('g')
    .selectAll('circle').data(visNodes).join('circle')
    .attr('r', d => nodeR(d, degree))
    .attr('fill', d => KIND_COLORS[d.kind] || '#888')
    .attr('stroke', '#1a1a1a')
    .attr('stroke-width', 1.5)
    .attr('cursor', 'pointer')
    .on('click', (_, d) => selectNode(d, degree, visEdges))
    .call(
      d3.drag()
        .on('start', (e, d) => { if (!e.active) simulation.alphaTarget(0.3).restart(); d.fx = d.x; d.fy = d.y; })
        .on('drag',  (e, d) => { d.fx = e.x; d.fy = e.y; })
        .on('end',   (e, d) => { if (!e.active) simulation.alphaTarget(0); d.fx = null; d.fy = null; })
    );

  nodeSel.append('title').text(d => d.name + ' (' + d.kind + ')\n' + d.file_path + ':' + d.line_start);

  // Labels for endpoints and high-degree nodes
  labelSel = g.append('g').attr('pointer-events', 'none')
    .selectAll('text').data(visNodes.filter(n => n.kind === 'endpoint' || (degree[n.id] || 0) >= 6))
    .join('text')
    .attr('font-size', 9)
    .attr('fill', '#ccc')
    .attr('text-anchor', 'middle')
    .attr('dy', d => -(nodeR(d, degree) + 3))
    .text(d => d.name.length > 24 ? d.name.slice(0, 22) + '…' : d.name);

  simulation.on('tick', () => {
    linkSel
      .attr('x1', d => d.source.x).attr('y1', d => d.source.y)
      .attr('x2', d => d.target.x).attr('y2', d => d.target.y);
    nodeSel.attr('cx', d => d.x).attr('cy', d => d.y);
    labelSel.attr('x', d => d.x).attr('y', d => d.y);
  });
}

function nodeR(node, degree) {
  return 4 + Math.sqrt(degree[node.id] || 0);
}

// ── Node detail panel ─────────────────────────────────────────────────────────
function selectNode(node, degree, edges) {
  selectedNode = node;
  const detail = document.getElementById('detail');
  detail.classList.add('open');

  const color = KIND_COLORS[node.kind] || '#888';
  const callers = edges.filter(e => e.kind === 'calls' && e.to === node.id)
    .map(e => allNodes.find(n => n.id === e.from)).filter(Boolean);
  const callees = edges.filter(e => e.kind === 'calls' && e.from === node.id)
    .map(e => allNodes.find(n => n.id === e.to)).filter(Boolean);

  const shortPath = node.file_path.replace(/\\\\/g, '/').split('/').slice(-3).join('/');

  detail.innerHTML =
    '<h3>' + esc(node.name) + '</h3>' +
    '<span class="kind-badge" style="background:' + color + '22;color:' + color + '">' + esc(node.kind) + '</span>' +

    '<div class="field"><div class="field-label">File</div>' +
    '<div class="field-val"><a id="openLink">' + esc(shortPath) + ':' + node.line_start + '</a></div></div>' +

    (node.detail ? '<div class="field"><div class="field-label">Detail</div><div class="field-val">' + esc(node.detail) + '</div></div>' : '') +

    (callers.length ? '<div class="field"><div class="field-label">Called by (' + callers.length + ')</div><ul>' +
      callers.map(n => '<li><a class="caller-link" data-id="' + n.id + '">' + esc(n.name) + '</a></li>').join('') +
      '</ul></div>' : '') +

    (callees.length ? '<div class="field"><div class="field-label">Calls (' + callees.length + ')</div><ul>' +
      callees.map(n => '<li><a class="callee-link" data-id="' + n.id + '">' + esc(n.name) + '</a></li>').join('') +
      '</ul></div>' : '');

  detail.querySelector('#openLink')?.addEventListener('click', () => {
    vscode.postMessage({ type: 'openFile', path: node.file_path, line: node.line_start });
  });
  detail.querySelectorAll('.caller-link, .callee-link').forEach(a => {
    a.addEventListener('click', () => {
      const target = allNodes.find(n => n.id === +a.dataset.id);
      if (target) {
        const deg = {};
        allEdges.forEach(e => { deg[e.from] = (deg[e.from]||0)+1; deg[e.to] = (deg[e.to]||0)+1; });
        selectNode(target, deg, allEdges);
        focusNode(target);
      }
    });
  });
}

function focusNode(node) {
  if (!svg || node.x === undefined) return;
  const svgEl = document.getElementById('svg');
  const W = svgEl.clientWidth || 800;
  const H = svgEl.clientHeight || 600;
  svg.transition().duration(400).call(
    d3.zoom().transform,
    d3.zoomIdentity.translate(W / 2 - node.x, H / 2 - node.y)
  );
}

// ── Controls ──────────────────────────────────────────────────────────────────
document.getElementById('fitBtn').addEventListener('click', () => {
  if (!svg || !g) return;
  const svgEl = document.getElementById('svg');
  const bounds = g.node().getBBox();
  const W = svgEl.clientWidth, H = svgEl.clientHeight;
  const scale = 0.85 / Math.max(bounds.width / W, bounds.height / H);
  const tx = W / 2 - scale * (bounds.x + bounds.width / 2);
  const ty = H / 2 - scale * (bounds.y + bounds.height / 2);
  svg.transition().duration(400).call(
    d3.zoom().transform,
    d3.zoomIdentity.translate(tx, ty).scale(scale)
  );
});

document.getElementById('browserBtn').addEventListener('click', () => {
  vscode.postMessage({ type: 'openBrowser' });
});

document.getElementById('search').addEventListener('input', e => {
  searchTerm = e.target.value.trim();
  rerenderGraph();
});

function esc(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

// Kick off
loadGraph();
</script>
</body>
</html>`;
    }
}
