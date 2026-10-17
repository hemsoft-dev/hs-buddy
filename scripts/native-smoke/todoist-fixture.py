"""Owned loopback Todoist API v1 fixture. No upstream or owner credentials.

Public startup requires actual disposable CI ownership. Unit tests inject a
private Host value without changing process environment. This fixture models
only the documented routes and command subset used by the native planner.
"""
from __future__ import annotations

import copy
import dataclasses
import datetime
import http.server
import json
import os
from pathlib import Path
import socket
import threading
import urllib.parse
import uuid

TOKEN = 'native-fixture-todoist-token'
NOW = '2026-11-02T05:30:00Z'
MARKER = '.buddy-todoist-fixture.json'
MAX_BODY = 256 * 1024


@dataclasses.dataclass(frozen=True)
class _Host:
    actions: str | None
    enabled: str | None
    temporary: str | None


def _regular(path):
    try:
        metadata = path.lstat()
    except OSError:
        return False
    return path.is_file() and not path.is_symlink() and not (getattr(metadata, 'st_file_attributes', 0) & 0x400)


def _due(date, recurring=False):
    return dict(date=date, string='every day' if recurring else date, timezone=None,
                is_recurring=recurring, lang='en')


def _task(identifier, content, project, date=None, priority=1, recurring=False):
    return dict(id=identifier, content=content, description='', project_id=project,
                parent_id=None, priority=priority, due=_due(date, recurring) if date else None,
                labels=['synthetic'], checked=False, is_deleted=False,
                added_at='2026-10-01T12:00:00Z', child_order=0)


class Fixture:
    @classmethod
    def start(cls, config):
        return cls._start_with_host(Path(config), _Host(os.environ.get('GITHUB_ACTIONS'),
            os.environ.get('BUDDY_TODOIST_FIXTURE'), os.environ.get('RUNNER_TEMP')))

    @classmethod
    def _start_with_host(cls, config, host):
        # Check explicit CI flags before reading a config or opening a socket.
        if host.actions != 'true' or host.enabled != '1' or not host.temporary:
            raise RuntimeError('Todoist fixture requires an owned disposable CI profile')
        root = Path(host.temporary)
        if not root.is_absolute() or not config.is_absolute() or not root.is_dir():
            raise RuntimeError('Todoist fixture profile ownership is unavailable')
        if root.is_symlink() or getattr(root, 'is_junction', lambda: False)() or not _regular(config):
            raise RuntimeError('Todoist fixture profile ownership is unavailable')
        if not config.resolve().is_relative_to(root.resolve()):
            raise RuntimeError('Todoist fixture profile is outside its disposable root')
        marker = config.resolve().parent / MARKER
        if marker.exists() or marker.is_symlink():
            raise RuntimeError('An existing Todoist fixture marker was preserved')
        return cls(marker)

    def __init__(self, marker):
        self.lock = threading.Lock()
        self.requests = []
        self.traffic = []
        self.expired = False
        self.reject_next_auth = False
        self.unavailable = False
        self.disconnect = False
        self.drop_next_ack = False
        self.commands = {}
        self.created = 0
        self.page_cap = 2
        self.fail_next_tasks = False
        self.tasks = [
            _task('task-overdue', 'Synthetic overdue task', 'project-alpha', '2026-11-01'),
            _task('task-today', 'Synthetic today task 界', 'project-alpha', '2026-11-02', 4),
            _task('task-upcoming', 'Synthetic upcoming task', 'project-alpha', '2026-11-03', 3),
            _task('task-undated', 'Synthetic undated task', 'project-beta'),
            _task('task-recurring', 'Synthetic recurring task', 'project-beta', '2026-11-02', recurring=True),
        ]
        self.projects = [dict(id='project-alpha', name='Synthetic Alpha', color='blue', parent_id=None,
            child_order=0, is_archived=False, is_deleted=False),
            dict(id='project-beta', name='Synthetic Beta', color='green', parent_id=None,
            child_order=1, is_archived=False, is_deleted=False),
            dict(id='project-empty', name='Synthetic Empty', color='grey', parent_id=None,
            child_order=2, is_archived=False, is_deleted=False)]
        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), _Handler)
        self.server.daemon_threads = True
        self.server.fixture = self
        self.url = f'http://127.0.0.1:{self.server.server_port}/api/v1/'
        self.marker = marker
        self.marker_bytes = json.dumps(dict(version=1, nonce=str(uuid.uuid4()), api_url=self.url, now_utc=NOW)).encode()
        try:
            descriptor = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, 'wb') as output:
                output.write(self.marker_bytes)
                output.flush()
                os.fsync(output.fileno())
        except BaseException:
            self.server.server_close()
            raise
        self.thread = threading.Thread(target=self.server.serve_forever,
            kwargs=dict(poll_interval=.05), name='buddy-todoist-owned-fixture', daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        if self.thread.is_alive():
            raise RuntimeError('The owned Todoist fixture did not stop')
        if self.marker.exists() and _regular(self.marker) and self.marker.read_bytes() == self.marker_bytes:
            self.marker.unlink()

    def snapshot(self):
        with self.lock:
            return copy.deepcopy(dict(tasks=self.tasks, requests=self.requests,
                                      traffic=self.traffic, created=self.created, projects=self.projects))

    def _get(self, route, query):
        if route == 'user':
            return dict(id='synthetic-todoist-account', tz_info=dict(timezone='America/Winnipeg'), is_deleted=False)
        if route not in ('tasks', 'tasks/filter', 'projects') or set(query) - {'limit', 'cursor', 'project_id', 'query'}:
            raise ValueError('Unsupported fixture route')
        rows = self.projects if route == 'projects' else [task for task in self.tasks if not task['checked'] and not task['is_deleted']]
        if 'project_id' in query:
            rows = [task for task in rows if task.get('project_id') == query['project_id'][0]]
        if route == 'tasks/filter':
            if query.get('query') != ['overdue | 7 days'] or 'project_id' in query:
                raise ValueError('Unsupported fixture calendar scope')
            # Fixed fixture date, independent of the host timezone database.
            rows = [task for task in rows if task.get('due') and task['due']['date'][:10] < '2026-11-09']
        elif 'query' in query:
            raise ValueError('Unsupported fixture filter')
        limit = min(int(query.get('limit', ['100'])[0]), self.page_cap)
        if limit <= 0:
            raise ValueError('Invalid fixture page limit')
        cursor = query.get('cursor', ['offset-0'])[0]
        if not cursor.startswith('offset-') or not cursor[7:].isdigit():
            raise ValueError('Invalid fixture cursor')
        offset = int(cursor[7:])
        end = offset + limit
        return dict(results=copy.deepcopy(rows[offset:end]), next_cursor=f'offset-{end}' if end < len(rows) else None)

    def _sync(self, commands):
        if not isinstance(commands, list) or len(commands) != 1:
            raise ValueError('One captured command is required')
        command = commands[0]
        if not isinstance(command, dict) or set(command) - {'type', 'uuid', 'temp_id', 'args'}:
            raise ValueError('Invalid fixture command')
        identifier = command['uuid']
        if not isinstance(identifier, str):
            raise ValueError('Invalid fixture command identity')
        parsed = uuid.UUID(identifier)
        if parsed.version != 4 or str(parsed) != identifier or not isinstance(command['args'], dict):
            raise ValueError('Invalid fixture command identity')
        if identifier in self.commands:
            _, reply = self.commands[identifier]
            # A duplicate UUID cannot execute new arguments. The controlling
            # test separately asserts that native retries retain exact bytes.
            return copy.deepcopy(reply)
        args = command['args']
        kind = command['type']
        project_id = args.get('project_id', 'project-alpha') if kind == 'item_add' else next(
            (task['project_id'] for task in self.tasks if task['id'] == args.get('id')), None)
        project = next((project for project in self.projects if project['id'] == project_id), None)
        role = project.get('role', 'READ_WRITE' if not project.get('workspace_id') else None) if project else None
        allowed = project is not None and not any(project.get(key) for key in ('is_archived', 'is_deleted', 'is_frozen')) and (
            role in ('CREATOR', 'ADMIN', 'READ_WRITE') or role == 'EDIT_ONLY' and kind == 'item_update'
            or role == 'COMPLETE_ONLY' and kind == 'item_close')
        if not allowed:
            reply = dict(sync_status={identifier: dict(error_code=403, error='Synthetic project permission denied')})
            self.commands[identifier] = (copy.deepcopy(command), copy.deepcopy(reply))
            return reply
        reply = dict(sync_status={identifier: 'ok'})
        if kind == 'item_add':
            temporary = command['temp_id']
            if not isinstance(temporary, str) or uuid.UUID(temporary).version != 4 or not isinstance(args['content'], str):
                raise ValueError('Invalid fixture task')
            next_created = self.created + 1
            task = _task(f'task-created-{next_created}', args['content'], args.get('project_id', 'project-alpha'))
            task['description'] = args.get('description', '')
            task['priority'] = args.get('priority', 1)
            task['due'] = _due(args['due']['date']) if args.get('due') else None
            self.created = next_created
            self.tasks.append(task)
            reply['temp_id_mapping'] = {temporary: task['id']}
        else:
            task = next((task for task in self.tasks if task['id'] == args.get('id') and not task['is_deleted']), None)
            if task is None:
                raise ValueError('The fixture task is unavailable')
            if kind == 'item_update':
                for key in ('content', 'description', 'priority'):
                    if key in args:
                        task[key] = copy.deepcopy(args[key])
                if 'due' in args:
                    task['due'] = _due(args['due']['date']) if args['due'] else None
            elif kind == 'item_close':
                if task['due'] and task['due']['is_recurring']:
                    day = datetime.date.fromisoformat(task['due']['date']) + datetime.timedelta(days=1)
                    task['due']['date'] = day.isoformat()
                else:
                    task['checked'] = True
            elif kind == 'item_uncomplete':
                task['checked'] = False
            elif kind == 'item_delete':
                task['is_deleted'] = True
            else:
                raise ValueError('Unsupported fixture command')
        self.commands[identifier] = (copy.deepcopy(command), copy.deepcopy(reply))
        return reply


class _Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self._handle(False)

    def do_POST(self):
        self._handle(True)

    def _respond(self, status, value):
        data = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Connection', 'close')
        self.end_headers()
        self.wfile.write(data)
        self.close_connection = True

    def _handle(self, write):
        self.connection.settimeout(3)
        fixture = self.server.fixture
        try:
            audit_path = urllib.parse.urlsplit(self.path).path
        except ValueError:
            audit_path = '<invalid-request-target>'
        # Count every HTTP attempt, including rejected auth and malformed bodies.
        # This allows a real zero-write assertion instead of counting only saves.
        with fixture.lock:
            fixture.traffic.append(dict(method=self.command, path=audit_path))
        body = None
        if write:
            try:
                length = int(self.headers.get('Content-Length', '0'))
                if not 0 < length <= MAX_BODY:
                    raise ValueError('Unsupported fixture request length')
                body = self.rfile.read(length)
                if len(body) != length:
                    raise ValueError('Incomplete fixture request')
            except (ValueError, OSError):
                self._respond(400, dict(error='Unsupported synthetic request'))
                return
        if self.headers.get('Authorization') != f'Bearer {TOKEN}':
            self._respond(401, dict(error='Synthetic authentication rejected'))
            return
        try:
            url = urllib.parse.urlsplit(self.path)
            if url.scheme or url.netloc or url.fragment or not url.path.startswith('/api/v1/'):
                raise ValueError('Unsupported fixture request')
            query = urllib.parse.parse_qs(url.query, strict_parsing=True)
            if any(len(values) != 1 for values in query.values()):
                raise ValueError('Ambiguous fixture query')
            with fixture.lock:
                if fixture.reject_next_auth:
                    fixture.reject_next_auth = False
                    self._respond(401, dict(error='Synthetic one-shot authentication rejected'))
                    return
                if fixture.disconnect:
                    self.close_connection = True
                    self.connection.shutdown(socket.SHUT_RDWR)
                    return
                if fixture.expired:
                    self._respond(401, dict(error='Synthetic authentication expired'))
                    return
                if fixture.unavailable:
                    self._respond(503, dict(error='Synthetic service unavailable'))
                    return
                route = url.path.removeprefix('/api/v1/')
                if not write and route in ('tasks', 'tasks/filter') and fixture.fail_next_tasks:
                    fixture.fail_next_tasks = False
                    self._respond(503, dict(error='Synthetic task read failed'))
                    return
                if write:
                    if route != 'sync' or query:
                        raise ValueError('Unsupported fixture mutation')
                    form = urllib.parse.parse_qs(body.decode(), strict_parsing=True)
                    if set(form) != {'commands'} or len(form['commands']) != 1:
                        raise ValueError('Invalid fixture command form')
                    commands = json.loads(form['commands'][0])
                    fixture.requests.append(dict(method='POST', path=url.path, commands=copy.deepcopy(commands)))
                    reply = fixture._sync(commands)
                    if fixture.drop_next_ack:
                        fixture.drop_next_ack = False
                        self.close_connection = True
                        self.connection.shutdown(socket.SHUT_RDWR)
                        return
                else:
                    fixture.requests.append(dict(method='GET', path=url.path, query=copy.deepcopy(query)))
                    reply = fixture._get(route, query)
            self._respond(200, reply)
        except (ValueError, KeyError, TypeError, StopIteration, OverflowError):
            self._respond(400, dict(error='Unsupported synthetic request'))
