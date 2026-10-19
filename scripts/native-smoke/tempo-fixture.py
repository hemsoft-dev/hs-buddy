"""Synthetic loopback service for future installed Tempo qualification.

No upstream client, real credentials, environment selector or native UI.
An operation UUID never deduplicates Tempo writes, including lost replies.
"""
import base64
from copy import deepcopy
from datetime import date, timedelta
from http.server import BaseHTTPRequestHandler
from loopback_http import LoopbackHTTPServer
import json
import socket
import threading
from urllib.parse import parse_qs, urlencode, urlsplit
import uuid

TEMPO_TOKEN = 'native-fixture-tempo-token'
JIRA_ORIGIN = 'https://native-tempo-jira.invalid/'
JIRA_EMAIL = 'native-tempo-fixture@example.invalid'
JIRA_TOKEN = 'native-fixture-jira-token'
ACCOUNT = '123456:synthetic-tempo-user'
ISSUE_ID = 9007199254740995
PROJECT_ID = 9007199254740997
FIRST_ID = 9007199254741993
NOW = '2026-10-16T16:00:00Z'
MAX_BODY = 1024 * 1024


def pairs(values):
    result = {}
    for key, value in values:
        if key in result:
            raise ValueError('Duplicate fixture field')
        result[key] = value
    return result


def positive(value):
    return type(value) is int and 0 < value <= 2**63 - 1


def exact_date(value):
    if not isinstance(value, str) or date.fromisoformat(value).isoformat() != value:
        raise ValueError('Invalid fixture date')
    return date.fromisoformat(value)


class TempoFixture:
    def __init__(self, tempo_origin='https://api.tempo.io'):
        if tempo_origin not in ('https://api.tempo.io', 'https://api.eu.tempo.io', 'https://api.us.tempo.io'):
            raise ValueError('Use a documented Tempo cloud origin')
        self.tempo_origin = tempo_origin
        self.attempts, self.writes = [], []
        self.fail_tempo = self.fail_jira = self.drop_next_create = False
        self.drop_next_delete = False
        self.fail_reads_after_write = False
        self.current_account = ACCOUNT
        self.current_timezone = 'America/New_York'
        self.capitalized = True
        self.fail_project_accounts = False
        self.missing_billing_account = False
        self.lock = threading.Lock()
        self.server = self.thread = None
        self.records = {}
        for index in range(51):
            identifier = FIRST_ID + index
            self.records[identifier] = dict(tempoWorklogId=identifier,
                issue=dict(id=ISSUE_ID, **{'self': JIRA_ORIGIN + 'rest/api/3/issue/' + str(ISSUE_ID)}),
                author=dict(accountId=ACCOUNT), timeSpentSeconds=3661 if index == 0 else 60,
                billableSeconds=3600 if index == 0 else 0, startDate='2026-10-16', startTime='09:00:00',
                description='Synthetic worklog 界' if index == 0 else f'Synthetic page row {index}',
                attributes={'self': f'{self.tempo_origin}/4/worklogs/{identifier}/work-attribute-values',
                    'values': [dict(key='_Account_', value='synthetic-billing-17'),
                        dict(key='_Unrelated_', value='preserve\n界'), dict(key='_Unset_')]},
                createdAt=NOW, updatedAt=NOW, **{'self': f'{self.tempo_origin}/4/worklogs/{identifier}'})
        self.next_id = FIRST_ID + 51

    def __enter__(self):
        if self.server is not None:
            raise RuntimeError('Use a fresh Tempo fixture')
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def setup(self):
                super().setup()
                self.connection.settimeout(3)

            def log_message(self, *_arguments):
                pass

            def handle_fixture(self):
                attempt = dict(method=self.command, path=self.path, bodyBase64='')
                with owner.lock:
                    owner.attempts.append(attempt)
                status, value = 400, {'error': 'Synthetic request rejected'}
                try:
                    lengths = self.headers.get_all('Content-Length', [])
                    if self.headers.get('Transfer-Encoding') is not None or len(lengths) > 1:
                        raise ValueError('Unsupported fixture body framing')
                    length = int(lengths[0]) if lengths else 0
                    if length < 0:
                        raise ValueError('Invalid fixture body length')
                    if length > MAX_BODY:
                        status = 413
                    else:
                        raw = self.rfile.read(length)
                        with owner.lock:
                            attempt['bodyBase64'] = base64.b64encode(raw).decode('ascii')
                        if len(raw) != length:
                            raise ValueError('Incomplete fixture body')
                        # Consume bounded bytes before authentication rejection.
                        # Otherwise a Windows client can observe a reset instead
                        # of the intended 401 for a body it is still sending.
                        with owner.lock:
                            status, value = owner.respond(self.command, self.path,
                                self.headers.get('Authorization'), raw)
                except (ValueError, TypeError, KeyError, UnicodeError, OSError):
                    pass
                if status is None:
                    self.close_connection = True
                    try:
                        self.connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    return
                body = b'' if status == 204 else json.dumps(value, ensure_ascii=False).encode('utf-8')
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.send_header('Connection', 'close')
                self.end_headers()
                self.wfile.write(body)

            do_GET = do_POST = do_PUT = do_DELETE = handle_fixture

        self.server = LoopbackHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = False
        self.url = f'http://127.0.0.1:{self.server.server_port}'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, _kind, primary, _traceback):
        try:
            self.server.shutdown()
            self.server.server_close()
            self.thread.join(timeout=5)
            if self.thread.is_alive():
                raise RuntimeError('The owned Tempo fixture did not stop')
        except BaseException as failure:
            if primary is None:
                raise
            primary.add_note(f'Tempo fixture cleanup failed: {type(failure).__name__}')

    def marker(self):
        if self.server is None:
            raise RuntimeError('Start the owned Tempo fixture first')
        return dict(version=1, nonce=str(uuid.uuid4()), tempo_origin=self.tempo_origin, tempo_url=self.url + '/4/',
            jira_url=self.url + '/', now_utc=NOW)

    def worklog(self, identifier, expanded=False):
        value = deepcopy(self.records[identifier])
        if not expanded:
            value['attributes'].pop('values')
        return value

    def respond(self, method, target, authorization, raw):
        route = urlsplit(target)
        path, query = route.path, parse_qs(route.query, keep_blank_values=True)
        jira = path.startswith('/rest/api/3/')
        expected = ('Basic ' + base64.b64encode(f'{JIRA_EMAIL}:{JIRA_TOKEN}'.encode()).decode()) if jira else 'Bearer ' + TEMPO_TOKEN
        if authorization != expected or (self.fail_jira if jira else self.fail_tempo):
            return 401, {'error': 'Synthetic authentication rejected'}
        if method == 'GET':
            if path.startswith('/4/') and self.fail_reads_after_write and self.writes:
                return 503, {'error': 'Synthetic refresh unavailable after acknowledgement'}
            if path == '/rest/api/3/myself' and not query:
                return 200, dict(accountId=self.current_account, timeZone=self.current_timezone)
            if path in ('/rest/api/3/issue/FIX-17', f'/rest/api/3/issue/{ISSUE_ID}'):
                if query == {'fields': ['id,key,summary']}:
                    return 200, dict(id=str(ISSUE_ID), key='FIX-17', fields=dict(summary='Synthetic issue 界'))
                if query == {'fields': ['id,key,customfield_11702,parent']}:
                    fields = {} if self.capitalized is None else dict(customfield_11702=dict(value='Yes' if self.capitalized else 'No'))
                    return 200, dict(id=str(ISSUE_ID), key='FIX-17', fields=fields)
                return 400, {'error': 'Synthetic issue fields required'}
            if path == '/rest/api/3/project/FIX' and not query:
                return 200, dict(id=str(PROJECT_ID), key='FIX')
            if path == f'/4/account-links/project/{PROJECT_ID}':
                if query != {'includeGlobalAccounts': ['true']}:
                    return 400, {'error': 'Synthetic project account scope required'}
                if self.fail_project_accounts:
                    return 503, {'error': 'Synthetic project accounts unavailable'}
                links = [dict(id=identifier, account={'self': f'{self.tempo_origin}/4/accounts/{identifier}'},
                    default=identifier == 17, scope=dict(id=PROJECT_ID, type='PROJECT')) for identifier in (17, 18, 19)]
                return 200, dict(metadata=dict(count=len(links)), results=links)
            if path == '/4/accounts':
                if query != {'limit': ['50'], 'offset': ['0']}:
                    return 400, {'error': 'Synthetic billing catalog pager required'}
                accounts = [dict(id=identifier, key=f'synthetic-billing-{identifier}', name=f'Synthetic billing account {identifier}',
                    global_=identifier == 19, status='CLOSED' if identifier == 18 else 'OPEN') for identifier in (17, 18, 19)
                    if not (self.missing_billing_account and identifier == 17)]
                for account in accounts:
                    account['global'] = account.pop('global_')
                return 200, dict(metadata=dict(count=len(accounts), limit=50, offset=0), results=accounts)
            if path in (f'/4/worklogs/user/{ACCOUNT}', f'/4/user-schedule/{ACCOUNT}'):
                if any(len(values) != 1 for values in query.values()):
                    raise ValueError('Duplicate fixture query')
                start, end = exact_date(query['from'][0]), exact_date(query['to'][0])
                if not 0 <= (end - start).days < 366:
                    raise ValueError('Invalid fixture range')
                if '/user-schedule/' in path:
                    if set(query) != {'from', 'to'}:
                        raise ValueError('Unexpected fixture query')
                    rows = []
                    for offset in range((end - start).days + 1):
                        day = start + timedelta(days=offset)
                        holiday = day.isoformat() == '2026-10-15'
                        working = day.weekday() < 5 and not holiday
                        row = dict(date=day.isoformat(), requiredSeconds=28800 if working else 0,
                            type='HOLIDAY' if holiday else 'WORKING_DAY' if working else 'NON_WORKING_DAY')
                        if holiday:
                            row['holiday'] = dict(name='Synthetic holiday')
                        rows.append(row)
                    return 200, dict(metadata=dict(count=len(rows)), results=rows,
                        **{'self': self.tempo_origin + target})
                if set(query) != {'from', 'to', 'limit', 'offset'} or query['limit'] != ['50']:
                    raise ValueError('Unexpected fixture pager')
                offset = int(query['offset'][0])
                if offset < 0:
                    raise ValueError('Invalid fixture offset')
                available = [identifier for identifier, row in sorted(self.records.items())
                    if start <= exact_date(row['startDate']) <= end]
                selected = available[offset:offset + 50]
                following = dict((key, values[0]) for key, values in query.items())
                following['offset'] = str(offset + 50)
                # The consumer must reconstruct the next loopback request from
                # captured scope. Never teach it to follow service URLs.
                next_page = (self.tempo_origin + path + '?' + urlencode(following)) if offset + 50 < len(available) else None
                return 200, dict(metadata=dict(count=len(selected), limit=50, offset=offset, next=next_page),
                    results=[self.worklog(identifier) for identifier in selected],
                    **{'self': self.tempo_origin + target})
            parts = path.strip('/').split('/')
            if len(parts) in (3, 4) and parts[:2] == ['4', 'worklogs'] and parts[2].isdigit() and not query:
                identifier = int(parts[2])
                if identifier not in self.records:
                    return 404, {'error': 'Synthetic worklog unavailable'}
                if len(parts) == 3:
                    return 200, self.worklog(identifier)
                if parts[3] == 'work-attribute-values':
                    values = deepcopy(self.records[identifier]['attributes']['values'])
                    return 200, dict(metadata=dict(count=len(values)), results=values,
                        **{'self': f'{self.tempo_origin}/4/worklogs/{identifier}/work-attribute-values'})
            return 404, {'error': 'Synthetic route unavailable'}
        if method == 'DELETE':
            parts = path.strip('/').split('/')
            if len(parts) != 3 or parts[:2] != ['4', 'worklogs'] or not parts[2].isdigit() or query or raw:
                return 400, {'error': 'Synthetic deletion target or body rejected'}
            identifier = int(parts[2])
            if identifier not in self.records:
                return 404, {'error': 'Synthetic worklog unavailable'}
            if self.records[identifier]['author']['accountId'] != ACCOUNT:
                return 403, {'error': 'Synthetic worklog scope rejected'}
            del self.records[identifier]
            self.writes.append(dict(method=method, worklogId=identifier, bodyBase64=''))
            if self.drop_next_delete:
                self.drop_next_delete = False
                return None, None
            return 204, None
        if method not in ('POST', 'PUT'):
            return 405, {'error': 'Synthetic method unavailable'}
        parts = path.strip('/').split('/')
        if query or (method == 'POST' and path != '/4/worklogs') or (method == 'PUT' and not (len(parts) == 3 and parts[:2] == ['4', 'worklogs'] and parts[2].isdigit())):
            return 404, {'error': 'Synthetic write route unavailable'}
        value = json.loads(raw.decode('utf-8'), object_pairs_hook=pairs)
        required = {'authorAccountId', 'startDate', 'timeSpentSeconds', 'attributes'}
        allowed = required | {'description', 'billableSeconds', 'startTime'}
        if method == 'POST':
            required |= {'issueId'}
            allowed |= {'issueId'}
        if not isinstance(value, dict) or not required <= set(value) <= allowed:
            raise ValueError('Unsupported fixture write')
        exact_date(value['startDate'])
        if value['authorAccountId'] != ACCOUNT or not positive(value['timeSpentSeconds']):
            raise ValueError('Wrong fixture scope')
        if method == 'POST' and (type(value['issueId']) is not int or value['issueId'] != ISSUE_ID):
            raise ValueError('Wrong fixture issue')
        if 'billableSeconds' in value and (type(value['billableSeconds']) is not int or not -2**63 <= value['billableSeconds'] < 2**63):
            raise ValueError('Invalid fixture billable seconds')
        if 'description' in value and not isinstance(value['description'], str):
            raise ValueError('Invalid fixture description')
        if 'startTime' in value:
            from datetime import time
            if not isinstance(value['startTime'], str) or time.fromisoformat(value['startTime']).isoformat() != value['startTime']:
                raise ValueError('Invalid fixture time')
        attributes = value['attributes']
        if not isinstance(attributes, list) or len(attributes) > 256:
            raise ValueError('Invalid fixture attributes')
        seen = set()
        for attribute in attributes:
            if not isinstance(attribute, dict) or not {'key'} <= set(attribute) <= {'key', 'value'}:
                raise ValueError('Invalid fixture attribute')
            key = attribute['key']
            if not isinstance(key, str) or not key or key in seen or ('value' in attribute and not isinstance(attribute['value'], str)):
                raise ValueError('Invalid fixture attribute value')
            seen.add(key)
        identifier = self.next_id if method == 'POST' else int(parts[2])
        if method == 'PUT' and identifier not in self.records:
            return 404, {'error': 'Synthetic worklog unavailable'}
        issue = ISSUE_ID if method == 'POST' else self.records[identifier]['issue']['id']
        record = dict(tempoWorklogId=identifier, issue=dict(id=issue, **{'self': JIRA_ORIGIN + 'rest/api/3/issue/' + str(issue)}),
            author=dict(accountId=ACCOUNT), startDate=value['startDate'], timeSpentSeconds=value['timeSpentSeconds'],
            attributes={'self': f'{self.tempo_origin}/4/worklogs/{identifier}/work-attribute-values', 'values': deepcopy(attributes)},
            createdAt=NOW, updatedAt=NOW, **{'self': f'{self.tempo_origin}/4/worklogs/{identifier}'})
        for key in ('description', 'billableSeconds', 'startTime'):
            if key in value:
                record[key] = value[key]
        self.records[identifier] = record
        if method == 'POST':
            self.next_id += 1
        self.writes.append(dict(method=method, path=path,
            bodyBase64=base64.b64encode(raw).decode('ascii'), worklogId=identifier))
        if method == 'POST' and self.drop_next_create:
            self.drop_next_create = False
            return None, None
        return 200, self.worklog(identifier, expanded=True)
