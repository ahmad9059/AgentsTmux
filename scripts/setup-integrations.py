#!/usr/bin/env python3
"""Install AgentsTmux bridges without replacing unrelated agent settings."""
import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
HOME = Path.home()
BACKUPS = HOME / '.local/state/agentstmux/backups'


def backup(path):
    if path.exists() or path.is_symlink():
        BACKUPS.mkdir(parents=True, exist_ok=True)
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
        destination = BACKUPS / f'{stamp}-{path.name}'
        if path.is_dir():
            raise RuntimeError(f'Refusing to replace an existing directory: {path}')
        shutil.copy2(path, destination, follow_symlinks=True)
        print(f'Backup: {destination}')


def load_json(path):
    if not path.exists():
        return {}
    result = json.loads(path.read_text())
    if not isinstance(result, dict):
        raise RuntimeError(f'Expected a JSON object in {path}')
    return result


def save_json(path, data):
    content = json.dumps(data, indent=2) + '\n'
    if path.exists() and path.read_text() == content:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    backup(path)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as temporary:
        temporary.write(content)
        filename = temporary.name
    os.chmod(filename, 0o600)
    os.replace(filename, path)


def link(source, destination):
    if destination.is_symlink() and destination.resolve() == source.resolve():
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    backup(destination)
    if destination.exists() or destination.is_symlink():
        destination.unlink()
    destination.symlink_to(source, target_is_directory=source.is_dir())


def binary():
    for path in [ROOT / 'bin/agents-tmux', ROOT / 'target/release/agents-tmux']:
        if path.is_file() and os.access(path, os.X_OK):
            return path
    raise RuntimeError('Build AgentsTmux first: cargo build --release')


def codex():
    path = Path(os.environ.get('CODEX_HOME', str(HOME / '.codex'))) / 'hooks.json'
    data = load_json(path)
    snippet = json.loads(subprocess.check_output([str(binary()), 'setup', 'codex'], text=True))
    hooks = data.setdefault('hooks', {})
    for event, entries in snippet['hooks'].items():
        existing = hooks.setdefault(event, [])
        for entry in entries:
            if entry not in existing:
                existing.append(entry)
    save_json(path, data)
    print('Codex: configured. Review and trust the new hooks with /hooks.')


def opencode():
    directory = Path(os.environ.get('XDG_CONFIG_HOME', str(HOME / '.config'))) / 'opencode'
    path = directory / 'cli.json'
    data = load_json(path)
    data.setdefault('$schema', 'https://opencode.ai/v2/cli.json')
    package = directory / 'agents-tmux'
    link(ROOT / 'integrations/opencode-v2', package)
    entry = {'package': str(package), 'options': {'hookPath': str(ROOT / 'hook.sh')}}
    plugins = data.setdefault('plugins', [])
    plugins[:] = [item for item in plugins if item != str(package) and not (isinstance(item, dict) and item.get('package') == str(package))]
    plugins.append(entry)
    save_json(path, data)
    print('OpenCode V2: configured. Use /reload if needed.')


def pi():
    directory = Path(os.environ.get('PI_CODING_AGENT_DIR', str(HOME / '.pi/agent')))
    link(ROOT / 'integrations/pi/agents-tmux.ts', directory / 'extensions/agents-tmux.ts')
    print('Pi: extension installed. Reload extensions or start a new session.')


def claude():
    if not shutil.which('claude'):
        raise RuntimeError('Claude Code CLI is not installed')
    subprocess.run(['claude', 'plugin', 'marketplace', 'add', str(ROOT)], check=True)
    subprocess.run(['claude', 'plugin', 'install', 'agents-tmux@agentstmux'], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['all', 'claude', 'codex', 'opencode', 'pi']:
        parser.add_argument('--' + name, action='store_true')
    options = parser.parse_args()
    if not any(vars(options).values()):
        parser.error('select --all or one or more agent flags')
    for name, install in [('codex', codex), ('opencode', opencode), ('pi', pi), ('claude', claude)]:
        if options.all or getattr(options, name):
            install()


if __name__ == '__main__':
    main()
