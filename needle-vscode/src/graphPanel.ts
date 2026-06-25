import * as vscode from 'vscode';
import * as fs from 'fs';

export class NeedleGraphPanel {
    public static currentPanel: NeedleGraphPanel | undefined;
    private static readonly viewType = 'needle.graph';

    private readonly _panel: vscode.WebviewPanel;
    private _disposables: vscode.Disposable[] = [];

    public static async createOrShow(extensionUri: vscode.Uri, serverUrl: string) {
        const col = vscode.window.activeTextEditor?.viewColumn ?? vscode.ViewColumn.One;
        if (NeedleGraphPanel.currentPanel) {
            NeedleGraphPanel.currentPanel._panel.reveal(col);
            await NeedleGraphPanel.currentPanel.load();
            return;
        }
        const panel = vscode.window.createWebviewPanel(
            NeedleGraphPanel.viewType,
            'Needle — Knowledge Graph',
            col,
            { enableScripts: true, retainContextWhenHidden: true,
              localResourceRoots: [vscode.Uri.joinPath(extensionUri, 'media')] }
        );
        NeedleGraphPanel.currentPanel = new NeedleGraphPanel(panel, extensionUri, serverUrl);
        await NeedleGraphPanel.currentPanel.load();
    }

    private constructor(
        panel: vscode.WebviewPanel,
        private readonly extensionUri: vscode.Uri,
        private readonly serverUrl: string,
    ) {
        this._panel = panel;
        this._panel.webview.onDidReceiveMessage(async msg => {
            if (msg.type === 'openFile' && msg.path) {
                vscode.commands.executeCommand('needle.openFile', msg.path, msg.line ?? 1);
            } else if (msg.type === 'openBrowser') {
                vscode.env.openExternal(vscode.Uri.parse(this.serverUrl));
            } else if (msg.type === 'reload') {
                await this.load();
            }
        }, null, this._disposables);
        this._panel.onDidDispose(() => this.dispose(), null, this._disposables);
        this._panel.webview.html = this.shellHtml('Loading graph…', false);
    }

    /** Fetch graph data in the extension host (proven path) and bake it into the HTML. */
    private async load() {
        try {
            const [gRes, sRes] = await Promise.all([
                fetch(this.serverUrl + '/api/graph',  { signal: AbortSignal.timeout(10000) }),
                fetch(this.serverUrl + '/api/status', { signal: AbortSignal.timeout(5000) }),
            ]);
            if (!gRes.ok) { throw new Error('needle serve returned ' + gRes.status); }
            const graph  = await gRes.json();
            const status = sRes.ok ? await sRes.json() : {};
            this._panel.webview.html = this.graphHtml(graph, status);
        } catch (e) {
            const message = e instanceof Error ? e.message : String(e);
            this._panel.webview.html = this.shellHtml(message, true);
        }
    }

    private dispose() {
        NeedleGraphPanel.currentPanel = undefined;
        this._panel.dispose();
        while (this._disposables.length) { this._disposables.pop()?.dispose(); }
    }

    /** Loading / error shell (no data). */
    private shellHtml(message: string, isError: boolean): string {
        const nonce = mkNonce();
        const icon = isError ? '⚠' : '⟳';
        const safe = esc(message);
        const retry = isError
            ? `<div style="font-family:monospace;background:rgba(127,127,127,.15);border:1px solid #333;border-radius:4px;padding:5px 14px;margin-top:6px">needle serve</div>
               <button id="rb" style="background:#7C3AED;color:#fff;border:none;border-radius:4px;padding:6px 18px;font-size:12px;cursor:pointer;margin-top:12px">Retry</button>`
            : '';
        return `<!DOCTYPE html><html><head>
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'nonce-${nonce}'; style-src 'unsafe-inline';">
<style>body{margin:0;height:100vh;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:14px;
background:var(--vscode-editor-background,#0c0c0d);color:var(--vscode-descriptionForeground);font-family:var(--vscode-font-family);font-size:13px;text-align:center;padding:24px}
.i{font-size:28px;opacity:.5}.m{max-width:380px;line-height:1.7}</style></head>
<body><div class="i">${icon}</div><div class="m">${safe}</div>${retry}
<script nonce="${nonce}">const v=acquireVsCodeApi();const b=document.getElementById('rb');if(b)b.onclick=()=>v.postMessage({type:'reload'});</script>
</body></html>`;
    }

    /** Full graph view with data embedded directly in the page. */
    private graphHtml(graph: unknown, status: unknown): string {
        const nonce = mkNonce();
        // Inline d3 source directly so it is guaranteed available — no external
        // file load, no CSP source matching, no URI resolution to go wrong.
        let d3Src = '';
        try {
            d3Src = fs.readFileSync(
                vscode.Uri.joinPath(this.extensionUri, 'media', 'd3.min.js').fsPath,
                'utf8',
            );
        } catch { /* d3 missing — graph area will stay empty but UI still loads */ }
        // Embed JSON safely (escape `<` so a "</script>" inside strings can't break out).
        const dataJson = JSON.stringify({ graph, status }).replace(/</g, '\\u003c');

        return `<!DOCTYPE html><html lang="en"><head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy"
  content="default-src 'none'; script-src 'nonce-${nonce}'; style-src 'unsafe-inline';">
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
body{background:var(--vscode-editor-background,#0c0c0d);color:var(--vscode-foreground);font-family:var(--vscode-font-family);font-size:var(--vscode-font-size);height:100vh;display:flex;flex-direction:column;overflow:hidden}
.toolbar{display:flex;align-items:center;gap:10px;padding:8px 12px;border-bottom:1px solid var(--vscode-panel-border,#333);flex-shrink:0;flex-wrap:wrap}
.toolbar h2{font-size:13px;font-weight:600}.stat{font-size:11px;color:var(--vscode-descriptionForeground)}.spacer{flex:1}
.filter-wrap{display:flex;gap:6px;flex-wrap:wrap}label{font-size:11px;display:flex;align-items:center;gap:3px;cursor:pointer}
input[type=checkbox]{accent-color:#7C3AED}
.si{background:var(--vscode-input-background);color:var(--vscode-input-foreground);border:1px solid var(--vscode-input-border,transparent);border-radius:4px;padding:3px 7px;font:inherit;font-size:12px;outline:none;width:170px}
.btn{background:var(--vscode-button-secondaryBackground,#2d2d30);color:var(--vscode-button-secondaryForeground,#ccc);border:1px solid transparent;border-radius:4px;padding:4px 10px;font:inherit;font-size:11px;cursor:pointer}
.main{display:flex;flex:1;overflow:hidden}#canvas{flex:1;overflow:hidden;position:relative}#canvas svg{width:100%;height:100%}
#detail{width:240px;flex-shrink:0;border-left:1px solid var(--vscode-panel-border,#333);overflow-y:auto;padding:12px;font-size:12px;display:none}
#detail.open{display:block}#detail h3{font-size:13px;margin-bottom:6px;word-break:break-all}
.kb{display:inline-block;padding:1px 6px;border-radius:10px;font-size:10px;margin-bottom:8px}.field{margin-bottom:6px}
.fl{color:var(--vscode-descriptionForeground);font-size:10px;text-transform:uppercase;letter-spacing:.05em;margin-bottom:2px}
.fv{word-break:break-all}.fv a,#detail ul li a{color:var(--vscode-textLink-foreground);cursor:pointer}#detail ul{padding-left:16px}
</style></head><body>
<div class="toolbar">
  <h2>Knowledge Graph</h2><span id="stats" class="stat"></span><div class="spacer"></div>
  <input id="search" class="si" type="search" placeholder="Filter nodes…">
  <div class="filter-wrap" id="filters"></div>
  <button class="btn" id="fitBtn">Fit</button>
  <button class="btn" id="browserBtn">Open in browser ↗</button>
</div>
<div class="main"><div id="canvas"><svg id="svg"></svg></div><div id="detail"></div></div>
<script nonce="${nonce}">${d3Src}</script>
<script nonce="${nonce}">
const vscode = acquireVsCodeApi();
const PAYLOAD = ${dataJson};
const COLORS={module:'#71717a',function:'#6366f1',method:'#3b82f6',class:'#8b5cf6',struct:'#f97316',trait:'#f59e0b',endpoint:'#10b981'};
const ALL_N=(PAYLOAD.graph.nodes)||[], ALL_E=(PAYLOAD.graph.edges)||[], STATS=(PAYLOAD.graph.stats)||{}, STATUS=PAYLOAD.status||{};
let filters=new Set(['function','method','class','struct','trait','endpoint','module']);
let term='', sim, svg, g, lSel, nSel, tSel;

document.getElementById('stats').textContent=(STATS.total_nodes||ALL_N.length)+' nodes · '+(STATS.total_edges||ALL_E.length)+' edges · '+(STATUS.total_files||0)+' files';
buildFilters(); render();

function buildFilters(){const w=document.getElementById('filters');
[['endpoint','Endpoints ('+(STATS.endpoints||0)+')'],['class','Classes ('+(STATS.classes||0)+')'],['function','Functions ('+(STATS.functions||0)+')'],['method','Methods ('+(STATS.methods||0)+')'],['module','Modules ('+(STATS.modules||0)+')']]
.forEach(([k,l])=>{const lb=document.createElement('label');lb.innerHTML='<input type="checkbox" value="'+k+'"'+(filters.has(k)?' checked':'')+'>'+l;
lb.querySelector('input').onchange=e=>{if(e.target.checked)filters.add(k);else filters.delete(k);rerender();};w.appendChild(lb);});}

function render(){svg=d3.select('#svg');svg.selectAll('*').remove();
svg.append('defs').append('marker').attr('id','arr').attr('viewBox','0 -4 8 8').attr('refX',14).attr('refY',0).attr('markerWidth',6).attr('markerHeight',6).attr('orient','auto').append('path').attr('d','M0,-4L8,0L0,4').attr('fill','#555');
g=svg.append('g');svg.call(d3.zoom().scaleExtent([0.05,4]).on('zoom',e=>g.attr('transform',e.transform)));rerender();}

function rerender(){if(!g)return;const t=term.toLowerCase();
const vn=ALL_N.filter(n=>filters.has(n.kind)&&(!t||n.name.toLowerCase().includes(t)||n.file_path.toLowerCase().includes(t)));
const vi=new Set(vn.map(n=>n.id));const ve=ALL_E.filter(e=>e.kind!=='contains'&&vi.has(e.from)&&vi.has(e.to));
const deg={};ve.forEach(e=>{deg[e.from]=(deg[e.from]||0)+1;deg[e.to]=(deg[e.to]||0)+1;});
const el=document.getElementById('svg');const W=el.clientWidth||900,H=el.clientHeight||600;if(sim)sim.stop();
sim=d3.forceSimulation(vn).force('link',d3.forceLink(ve).id(d=>d.id).distance(60).strength(0.5)).force('charge',d3.forceManyBody().strength(-120)).force('center',d3.forceCenter(W/2,H/2)).force('col',d3.forceCollide().radius(d=>r(d,deg)+4));
g.selectAll('*').remove();
lSel=g.append('g').attr('opacity',0.4).selectAll('line').data(ve).join('line').attr('stroke',d=>d.kind==='imports'?'#6366f1':'#555').attr('stroke-width',1).attr('marker-end','url(#arr)');
nSel=g.append('g').selectAll('circle').data(vn).join('circle').attr('r',d=>r(d,deg)).attr('fill',d=>COLORS[d.kind]||'#888').attr('stroke','#1a1a1a').attr('stroke-width',1.5).attr('cursor','pointer').on('click',(_,d)=>pick(d,deg,ve))
.call(d3.drag().on('start',(e,d)=>{if(!e.active)sim.alphaTarget(0.3).restart();d.fx=d.x;d.fy=d.y;}).on('drag',(e,d)=>{d.fx=e.x;d.fy=e.y;}).on('end',(e,d)=>{if(!e.active)sim.alphaTarget(0);d.fx=null;d.fy=null;}));
nSel.append('title').text(d=>d.name+' ('+d.kind+')\\n'+d.file_path+':'+d.line_start);
tSel=g.append('g').attr('pointer-events','none').selectAll('text').data(vn.filter(n=>n.kind==='endpoint'||(deg[n.id]||0)>=6)).join('text').attr('font-size',9).attr('fill','#ccc').attr('text-anchor','middle').attr('dy',d=>-(r(d,deg)+3)).text(d=>d.name.length>24?d.name.slice(0,22)+'…':d.name);
sim.on('tick',()=>{lSel.attr('x1',d=>d.source.x).attr('y1',d=>d.source.y).attr('x2',d=>d.target.x).attr('y2',d=>d.target.y);nSel.attr('cx',d=>d.x).attr('cy',d=>d.y);tSel.attr('x',d=>d.x).attr('y',d=>d.y);});}

function r(n,deg){return 4+Math.sqrt(deg[n.id]||0);}

function pick(node,deg,edges){const dt=document.getElementById('detail');dt.classList.add('open');const c=COLORS[node.kind]||'#888';
const cr=edges.filter(e=>e.kind==='calls'&&e.to===node.id).map(e=>ALL_N.find(n=>n.id===e.from)).filter(Boolean);
const ce=edges.filter(e=>e.kind==='calls'&&e.from===node.id).map(e=>ALL_N.find(n=>n.id===e.to)).filter(Boolean);
const sp=node.file_path.replace(/\\\\/g,'/').split('/').slice(-3).join('/');
dt.innerHTML='<h3>'+esc(node.name)+'</h3><span class="kb" style="background:'+c+'22;color:'+c+'">'+esc(node.kind)+'</span>'+
'<div class="field"><div class="fl">File</div><div class="fv"><a id="ol">'+esc(sp)+':'+node.line_start+'</a></div></div>'+
(cr.length?'<div class="field"><div class="fl">Called by ('+cr.length+')</div><ul>'+cr.map(n=>'<li><a class="cl" data-id="'+n.id+'">'+esc(n.name)+'</a></li>').join('')+'</ul></div>':'')+
(ce.length?'<div class="field"><div class="fl">Calls ('+ce.length+')</div><ul>'+ce.map(n=>'<li><a class="cl" data-id="'+n.id+'">'+esc(n.name)+'</a></li>').join('')+'</ul></div>':'');
dt.querySelector('#ol').onclick=()=>vscode.postMessage({type:'openFile',path:node.file_path,line:node.line_start});
dt.querySelectorAll('.cl').forEach(a=>a.onclick=()=>{const x=ALL_N.find(n=>n.id===+a.dataset.id);if(x){pick(x,deg,ALL_E);}});}

document.getElementById('fitBtn').onclick=()=>{if(!svg||!g)return;const el=document.getElementById('svg'),b=g.node().getBBox();const W=el.clientWidth,H=el.clientHeight;const sc=0.85/Math.max(b.width/W,b.height/H);svg.transition().duration(400).call(d3.zoom().transform,d3.zoomIdentity.translate(W/2-sc*(b.x+b.width/2),H/2-sc*(b.y+b.height/2)).scale(sc));};
document.getElementById('browserBtn').onclick=()=>vscode.postMessage({type:'openBrowser'});
document.getElementById('search').addEventListener('input',e=>{term=e.target.value.trim();rerender();});
function esc(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');}
</script></body></html>`;
    }
}

function mkNonce(): string {
    return Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
}
function esc(s: string): string {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
