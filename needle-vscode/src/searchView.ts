import * as vscode from 'vscode';

interface SearchResult {
    file_path: string;
    line_start: number;
    line_end: number;
    language: string;
    content: string;
    score: number;
    signal: string;
}

interface SearchResponse {
    results: SearchResult[];
    timing?: { total_ms: number };
    total?: number;
}

export class NeedleSearchViewProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'needle.search';

    constructor(
        private readonly extensionUri: vscode.Uri,
        private readonly serverUrl: () => string,
    ) {}

    resolveWebviewView(
        view: vscode.WebviewView,
        _ctx: vscode.WebviewViewResolveContext,
        _token: vscode.CancellationToken,
    ) {
        view.webview.options = { enableScripts: true };
        view.webview.html = this.buildHtml();

        view.webview.onDidReceiveMessage(async (msg: { type: string; query?: string; path?: string; line?: number }) => {
            if (msg.type === 'search' && msg.query) {
                try {
                    const url = `${this.serverUrl()}/api/search?q=${encodeURIComponent(msg.query)}&limit=15`;
                    const res = await fetch(url, { signal: AbortSignal.timeout(5000) });
                    if (!res.ok) { throw new Error(`Server returned ${res.status}`); }
                    const data = await res.json() as SearchResponse;
                    view.webview.postMessage({ type: 'results', results: data.results ?? [], timing: data.timing });
                } catch (e: unknown) {
                    const msg2 = e instanceof Error ? e.message : 'Search failed';
                    view.webview.postMessage({ type: 'error', message: msg2 });
                }
            }

            if (msg.type === 'openFile' && msg.path !== undefined && msg.line !== undefined) {
                vscode.commands.executeCommand('needle.openFile', msg.path, msg.line);
            }
        });
    }

    private buildHtml(): string {
        // Random nonce so each load gets a fresh inline script approval
        const nonce = Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);

        return /* html */`<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy"
  content="default-src 'none'; script-src 'nonce-${nonce}'; style-src 'unsafe-inline';">
<title>Needle Search</title>
<style>
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

  body {
    background: transparent;
    color: var(--vscode-foreground);
    font-family: var(--vscode-font-family);
    font-size: var(--vscode-font-size);
    padding: 8px 10px;
  }

  /* ── Search bar ── */
  .bar {
    display: flex;
    gap: 6px;
    margin-bottom: 6px;
  }
  input {
    flex: 1;
    background: var(--vscode-input-background);
    color: var(--vscode-input-foreground);
    border: 1px solid var(--vscode-input-border, transparent);
    border-radius: 4px;
    padding: 5px 8px;
    font: inherit;
    outline: none;
  }
  input:focus { border-color: var(--vscode-focusBorder); }
  input::placeholder { color: var(--vscode-input-placeholderForeground); }

  /* ── Meta line ── */
  .meta {
    font-size: 11px;
    color: var(--vscode-descriptionForeground);
    margin-bottom: 8px;
    min-height: 16px;
  }

  /* ── Result card ── */
  .result {
    margin-bottom: 10px;
    cursor: pointer;
    border-radius: 4px;
    border: 1px solid transparent;
    padding: 6px 8px;
    transition: background 0.1s;
  }
  .result:hover {
    background: var(--vscode-list-hoverBackground);
    border-color: var(--vscode-list-hoverBackground);
  }

  .result-header {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-bottom: 4px;
    overflow: hidden;
  }
  .badge {
    flex-shrink: 0;
    font-size: 10px;
    padding: 0 4px;
    border-radius: 3px;
    background: var(--vscode-badge-background);
    color: var(--vscode-badge-foreground);
  }
  .badge.sem  { background: #5b21b6; color: #ede9fe; }
  .badge.bm25 { background: #0e7490; color: #e0f2fe; }
  .badge.both { background: #065f46; color: #d1fae5; }

  .result-path {
    font-size: 11px;
    color: var(--vscode-textLink-foreground);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result-code {
    font-family: var(--vscode-editor-font-family, 'Courier New', monospace);
    font-size: 11px;
    line-height: 1.5;
    background: var(--vscode-textBlockQuote-background, rgba(127,127,127,0.1));
    border-left: 2px solid var(--vscode-textBlockQuote-border, #7C3AED);
    padding: 4px 6px;
    border-radius: 0 3px 3px 0;
    white-space: pre;
    overflow: hidden;
    max-height: 66px;
    color: var(--vscode-editor-foreground);
  }

  /* ── States ── */
  .empty, .hint {
    text-align: center;
    margin-top: 32px;
    font-size: 12px;
    color: var(--vscode-descriptionForeground);
    line-height: 1.8;
  }
  .error {
    font-size: 12px;
    color: var(--vscode-errorForeground);
    margin-top: 8px;
    padding: 6px 8px;
    background: var(--vscode-inputValidation-errorBackground, rgba(255,0,0,0.1));
    border-radius: 4px;
  }
</style>
</head>
<body>

<div class="bar">
  <input id="q" type="search" placeholder="Search code…" autocomplete="off" spellcheck="false">
</div>
<div id="meta" class="meta"></div>
<div id="results">
  <div class="hint">Type to search across the indexed codebase.<br>Results open the file at the exact line.</div>
</div>

<script nonce="${nonce}">
  const vscode = acquireVsCodeApi();
  const input  = document.getElementById('q');
  const meta   = document.getElementById('meta');
  const results = document.getElementById('results');
  let debounce;

  input.addEventListener('input', () => {
    clearTimeout(debounce);
    const q = input.value.trim();
    if (!q) {
      meta.textContent = '';
      results.innerHTML = '<div class="hint">Type to search across the indexed codebase.<br>Results open the file at the exact line.</div>';
      return;
    }
    meta.textContent = 'Searching…';
    debounce = setTimeout(() => send(q), 220);
  });

  input.addEventListener('keydown', e => {
    if (e.key === 'Enter') { clearTimeout(debounce); const q = input.value.trim(); if (q) send(q); }
    if (e.key === 'Escape') { input.value = ''; input.dispatchEvent(new Event('input')); }
  });

  function send(q) { vscode.postMessage({ type: 'search', query: q }); }

  window.addEventListener('message', e => {
    const msg = e.data;

    if (msg.type === 'results') {
      const r = msg.results || [];
      const ms = msg.timing ? msg.timing.total_ms.toFixed(1) : '?';
      meta.textContent = r.length + ' result' + (r.length === 1 ? '' : 's') + '  ·  ' + ms + 'ms';

      if (!r.length) {
        results.innerHTML = '<div class="empty">No results found.</div>';
        return;
      }

      results.innerHTML = r.map((item) => {
        const parts = item.file_path.replace(/\\\\/g, '/').split('/');
        const fname = parts.pop() || item.file_path;
        const dir   = parts.slice(-2).join('/');
        const sig   = (item.signal || '').toUpperCase();
        const badgeCls = sig === 'SEM' ? 'sem' : sig === 'BM25' ? 'bm25' : 'both';
        const badgeLbl = sig === 'SEM' ? 'SEM' : sig === 'BM25' ? 'BM25' : 'BOTH';
        const code  = esc(item.content.trim().slice(0, 300));
        return (
          '<div class="result" data-path="' + esc(item.file_path) + '" data-line="' + item.line_start + '">' +
            '<div class="result-header">' +
              '<span class="badge ' + badgeCls + '">' + badgeLbl + '</span>' +
              '<span class="badge">' + esc(item.language) + '</span>' +
              '<span class="result-path">' + esc(dir ? dir + '/' + fname : fname) + ':' + item.line_start + '</span>' +
            '</div>' +
            '<div class="result-code">' + code + '</div>' +
          '</div>'
        );
      }).join('');

      results.querySelectorAll('.result').forEach(el => {
        el.addEventListener('click', () => {
          vscode.postMessage({ type: 'openFile', path: el.dataset.path, line: +el.dataset.line });
        });
      });
    }

    if (msg.type === 'error') {
      meta.textContent = '';
      results.innerHTML = '<div class="error">⚠ ' + esc(msg.message) + '<br>Is <code>needle serve</code> running?</div>';
    }
  });

  function esc(s) {
    return String(s)
      .replace(/&/g,'&amp;')
      .replace(/</g,'&lt;')
      .replace(/>/g,'&gt;')
      .replace(/"/g,'&quot;');
  }
</script>
</body>
</html>`;
    }
}
