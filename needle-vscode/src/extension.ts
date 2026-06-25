import * as vscode from 'vscode';
import { NeedleSearchViewProvider } from './searchView';
import { NeedleGraphPanel } from './graphPanel';

export function activate(context: vscode.ExtensionContext) {
    const serverUrl = (): string =>
        vscode.workspace.getConfiguration('needle').get('serverUrl', 'http://localhost:7700');

    // ── Search sidebar ────────────────────────────────────────────────────────
    const searchProvider = new NeedleSearchViewProvider(context.extensionUri, serverUrl);
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(NeedleSearchViewProvider.viewType, searchProvider)
    );

    // ── Status bar ────────────────────────────────────────────────────────────
    const bar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    bar.command = 'needle.showGraph';
    bar.show();
    context.subscriptions.push(bar);

    const refreshStatus = async () => {
        try {
            const res = await fetch(`${serverUrl()}/api/status`, {
                signal: AbortSignal.timeout(3000),
            });
            if (!res.ok) { throw new Error(); }
            const d = await res.json() as { total_chunks?: number; total_files?: number };
            const chunks = d.total_chunks ?? 0;
            const files  = d.total_files  ?? 0;
            bar.text = `$(search) Needle: ${chunks} chunks`;
            bar.tooltip = `${files} files indexed — click to open knowledge graph`;
            bar.backgroundColor = undefined;
            searchProvider.pushStatus(true, chunks, files);
        } catch {
            bar.text = `$(warning) Needle: offline`;
            bar.tooltip = `needle serve is not running on ${serverUrl()}`;
            bar.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
            searchProvider.pushStatus(false, 0, 0);
        }
    };

    refreshStatus();
    const timer = setInterval(refreshStatus, 30_000);
    context.subscriptions.push({ dispose: () => clearInterval(timer) });

    // ── Commands ──────────────────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('needle.showGraph', () =>
            NeedleGraphPanel.createOrShow(context.extensionUri, serverUrl())
        ),
        vscode.commands.registerCommand('needle.openInBrowser', () =>
            vscode.env.openExternal(vscode.Uri.parse(serverUrl()))
        ),
        vscode.commands.registerCommand('needle.openFile', async (path: string, line: number) => {
            try {
                const uri = vscode.Uri.file(path);
                const doc = await vscode.workspace.openTextDocument(uri);
                await vscode.window.showTextDocument(doc, {
                    selection: new vscode.Range(
                        Math.max(0, line - 1), 0,
                        Math.max(0, line - 1), 0
                    ),
                    preview: false,
                });
            } catch {
                vscode.window.showErrorMessage(`Needle: cannot open ${path}`);
            }
        })
    );
}

export function deactivate() {}
