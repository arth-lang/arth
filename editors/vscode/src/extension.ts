import * as path from 'path';
import * as fs from 'fs';
import * as vscode from 'vscode';
import { LanguageClient, LanguageClientOptions, ServerOptions, Executable } from 'vscode-languageclient/node';

let client: LanguageClient | undefined;
let fileWatcher: vscode.FileSystemWatcher | undefined;

function candidateServerPaths(context: vscode.ExtensionContext): string[] {
  const configPath = vscode.workspace.getConfiguration().get<string>('arth-lsp.serverPath') || '';
  const exeName = process.platform === 'win32' ? 'arth-lsp.exe' : 'arth-lsp';

  const paths: string[] = [];

  if (configPath.trim().length > 0) {
    paths.push(configPath);
  }

  // If the workspace is the repo root
  for (const folder of vscode.workspace.workspaceFolders || []) {
    const w = folder.uri.fsPath;
    paths.push(path.join(w, 'target', 'debug', exeName));
    paths.push(path.join(w, 'target', 'release', exeName));
  }

  // If the extension is opened standalone, try repo root one level up
  const repoRoot = path.resolve(context.extensionPath, '..');
  paths.push(path.join(repoRoot, 'target', 'debug', exeName));
  paths.push(path.join(repoRoot, 'target', 'release', exeName));

  // Final fallback to PATH
  paths.push(exeName);
  return paths;
}

function resolveServerExecutable(context: vscode.ExtensionContext): Executable {
  const paths = candidateServerPaths(context);
  for (const p of paths) {
    try {
      // Only check existence for absolute or relative file paths
      if (p.includes(path.sep) && fs.existsSync(p)) {
        return { command: p, args: [], options: { env: process.env } };
      }
    } catch {
      // ignore
    }
  }
  // Let the OS resolve from PATH as last resort
  const fallback = paths[paths.length - 1];
  return { command: fallback, args: [], options: { env: process.env } };
}

async function startClient(context: vscode.ExtensionContext) {
  const serverExecutable = resolveServerExecutable(context);

  const documentSelector = [
    { scheme: 'file', language: 'arth' },
    { scheme: 'untitled', language: 'arth' },
  ];

  // Create a single FileSystemWatcher and reuse it across restarts to avoid leaks.
  if (!fileWatcher) {
    fileWatcher = vscode.workspace.createFileSystemWatcher('**/*.arth');
    // Ensure it gets disposed when the extension deactivates
    context.subscriptions.push(fileWatcher);
  }

  const clientOptions: LanguageClientOptions = {
    documentSelector,
    synchronize: {
      fileEvents: fileWatcher,
    },
  };

  const serverOptions: ServerOptions = serverExecutable;

  client = new LanguageClient('arthLsp', 'Arth Language Server', serverOptions, clientOptions);
  await client.start();

  const chosen = (serverExecutable as Executable).command;
  vscode.window.showInformationMessage(`Arth LSP: started (${chosen})`);
}

export async function activate(context: vscode.ExtensionContext) {
  await startClient(context);

  const restart = vscode.commands.registerCommand('arth.restartServer', async () => {
    if (client) {
      await client.stop();
      // Dispose to ensure resources are cleaned up
      client.dispose();
      client = undefined;
    }
    await startClient(context);
  });

  context.subscriptions.push(restart);

  // Lightweight semantic tokens: color builtins as function(builtin)
  const tokenTypes = ['function'];
  const tokenModifiers = ['declaration', 'static', 'builtin'];
  const legend = new vscode.SemanticTokensLegend(tokenTypes, tokenModifiers);
  const BUILTINS = [
    'println','print','assert','spawn','spawnBlocking','startTask','spawnTask','send','trySend','offer','post','emit'
  ];
  const builtinPattern = new RegExp(
    `\\b(?:${BUILTINS.map(x => x.replace(/[-/\\^$*+?.()|[\]{}]/g, '\\$&')).join('|')})\\b(?=\\s*\\()`,
    'g'
  );
  const provider: vscode.DocumentSemanticTokensProvider = {
    provideDocumentSemanticTokens(doc, _token) {
      const builder = new vscode.SemanticTokensBuilder(legend);
      for (let line = 0; line < doc.lineCount; line++) {
        const text = doc.lineAt(line).text;
        builtinPattern.lastIndex = 0;
        let m: RegExpExecArray | null;
        while ((m = builtinPattern.exec(text)) !== null) {
          const start = m.index;
          const len = m[0].length;
          builder.push(new vscode.Range(line, start, line, start + len), 'function', ['builtin']);
        }
      }
      return builder.build();
    },
  };
  context.subscriptions.push(
    vscode.languages.registerDocumentSemanticTokensProvider({ language: 'arth' }, provider, legend)
  );
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client.dispose();
    client = undefined;
  }
}
