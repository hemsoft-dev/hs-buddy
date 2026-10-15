"""Synthetic GitHub API for installed native qualification; never forwards traffic."""
from datetime import date, timedelta
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import re
import threading
from urllib.parse import urlparse, parse_qs

NOW = "2026-10-15T12:00:00Z"
SHA = "a" * 40
REPO = "fixture-org/native-demo"


def connection(nodes, more=False):
    return dict(nodes=nodes, pageInfo=dict(hasNextPage=more, endCursor="fixture-next" if more else None))


def pull_data(field, value):
    return {"data": {"repository": {"pullRequest": {field: value}}}}


def user(login="fixture-author"):
    return dict(login=login, name="Demo Contributor", type="User", html_url="https://github.com/" + login,
        bio="Synthetic profile for native qualification", company="Fixture Company", location="Fixture City",
        email="author@example.test", created_at=NOW, public_repos=4, followers=8, following=3)


def repository(name="native-demo"):
    return dict(full_name="fixture-org/" + name, description="Synthetic repository for installed qualification",
        language="Rust", default_branch="main", visibility="private", private=True, archived=False, fork=False,
        stargazers_count=12, forks_count=2, subscribers_count=3, open_issues_count=4, size=128,
        updated_at=NOW, pushed_at=NOW, homepage=None, license=dict(name="MIT"), topics=["fixture"])


def pull(account, number=7, approved=False, merged=False):
    return dict(number=number, title=("First private" if account == "fixture-one" else "Second private") + " navigation review",
        state="MERGED" if merged else "OPEN", body="Synthetic PR description. This account's data is isolated.",
        createdAt=NOW, updatedAt=NOW, mergedAt=NOW if merged else None, headRefName="feature/navigation",
        baseRefName="main", headRefOid=SHA, changedFiles=2, isDraft=False, reviewDecision="APPROVED" if approved else "REVIEW_REQUIRED",
        author=dict(login="fixture-author"), repository=dict(nameWithOwner=REPO),
        latestOpinion=dict(nodes=[dict(state="APPROVED")] if approved else []),
        recentReviews=dict(nodes=[dict(state="APPROVED", author=dict(login=account))] if approved else [], pageInfo=dict(hasPreviousPage=False)),
        reviewThreads=dict(totalCount=1))


def comment(identifier="IC_fixture", body="Please keep keyboard navigation available.", author="fixture-author"):
    return dict(id=identifier, body=body, author=dict(login=author), createdAt=NOW, updatedAt=NOW,
        url="https://github.com/" + REPO + "/pull/7#issuecomment-1", diffHunk=None,
        reactionGroups=[dict(content="THUMBS_UP", users=dict(totalCount=1))])


def issue(number=11):
    return dict(number=number, title="Fixture keyboard follow-up", state="open", body="Synthetic issue body",
        user=dict(login="fixture-author"), created_at=NOW, updated_at=NOW, closed_at=None, state_reason=None,
        milestone=None, labels=[dict(name="enhancement")], assignees=[], comments=1)


def file_diff(name="src/navigation.rs", patch="@@ -1 +1 @@\n-old navigation\n+native navigation"):
    return dict(filename=name, status="modified", additions=1, deletions=1, changes=2, patch=patch,
        blob_url="https://github.com/" + REPO + "/blob/" + SHA + "/" + name)


def commit():
    return dict(sha=SHA, commit=dict(message="Improve native navigation", author=dict(name="Fixture Author", date=NOW), committer=dict(date=NOW)),
        author=dict(login="fixture-author"), html_url="https://github.com/" + REPO + "/commit/" + SHA,
        parents=[], stats=dict(additions=1, deletions=1, total=2), files=[file_diff()])


class GitHubBackend:
    def __init__(self):
        self.requests = []
        self.comments = {account: [] for account in ("fixture-one", "fixture-two")}
        self.scenario = "normal"
        self.lock = threading.Lock()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                self.handle_api(None)

            def do_POST(self):
                length = int(self.headers.get("Content-Length", "0"))
                if length > 512 * 1024:
                    self.send_error(413)
                    return
                try:
                    body = json.loads(self.rfile.read(length))
                except (ValueError, UnicodeError):
                    self.send_error(400)
                    return
                self.handle_api(body)

            def handle_api(self, body):
                authorization = self.headers.get("Authorization", "")
                accounts = {"Bearer native-fixture-fixture-one": "fixture-one", "Bearer native-fixture-fixture-two": "fixture-two"}
                account = accounts.get(authorization)
                # Record identity, never an arbitrary authorization value.
                with owner.lock:
                    owner.requests.append(dict(method=self.command, path=self.path, account=account,
                        query=(body or {}).get("query", "").split("(")[0], body=body if self.path != "/graphql" else None))
                    status, response, headers = owner.respond(account, self.command, self.path, body)
                payload = json.dumps(response).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                for key, value in headers.items():
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(payload)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}/"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def respond(self, account, method, path, body):
        def ok(value, headers=None):
            return 200, value, headers or {}
        parsed = urlparse(path)
        route, params = parsed.path, parse_qs(parsed.query)
        if account is None or self.scenario == "auth" and account == "fixture-two":
            return 401, dict(message="Fixture authentication failure"), {}
        variables = (body or {}).get("variables", {})
        repo = variables.get("repo", "")
        if "/limited" in route or repo == "limited":
            return 429, dict(message="Fixture rate limit"), {"Retry-After": "60"}
        if "/denied" in route or repo == "denied":
            return 403, dict(message="Fixture permission denied"), {}
        if route == "/graphql":
            query = body.get("query", "")
            if "NativeViewerPulls(" in query:
                authored = dict(pull(account), author=dict(login=account))
                return ok(dict(data=dict(viewer=dict(login=account, pullRequests=connection([] if self.scenario == "empty" else [authored])))))
            if "NativePullRequests(" in query:
                search = variables["query"]
                nodes = [] if self.scenario in ("empty", "search-degraded") else [pull(account, approved="reviewed-by:" in search, merged="is:merged" in search)]
                more = "author:" in search and "is:open" in search and not variables.get("after") and bool(nodes)
                if variables.get("after"):
                    nodes = [dict(pull(account, 8), title="Paginated fixture follow-up")]
                return ok(dict(data=dict(search=dict(connection(nodes, more), issueCount=2 if more else len(nodes)))))
            if "NativeRepositoryCounts(" in query:
                return ok(dict(data=dict(repository=dict(issues=dict(totalCount=2), pullRequests=dict(totalCount=2)))))
            if "NativeUserContributions(" in query:
                days = [dict(date=str(date(2026, 10, 15) - timedelta(days=364-index)), contributionCount=index % 6, color="#238636") for index in range(365)]
                return ok(dict(data=dict(user=dict(status=dict(message="Working on native Buddy", emoji=""), contributionsCollection=dict(contributionCalendar=dict(totalContributions=sum(day["contributionCount"] for day in days), weeks=[dict(contributionDays=days[i:i+7]) for i in range(0,365,7)]))))))
            if "NativeUserTeams(" in query:
                return ok(dict(data=dict(organization=dict(teams=connection([dict(slug="native-team", name="Native Team", description="Fixture team", url="https://github.com/orgs/fixture-org/teams/native-team")])))))
            if "NativeThreadComments(" in query:
                return ok(dict(data=dict(node=dict(pullRequest=dict(number=7, repository=dict(nameWithOwner=REPO)), comments=connection([comment()])))))
            if "NativePullThreads(" in query:
                value = connection([dict(id="PRRT_fixture", isResolved=False, isOutdated=False, path="src/navigation.rs", line=1, diffSide="RIGHT", comments=connection([comment()]))])
                return ok(dict(data=dict(repository=dict(pullRequest=dict(reviewThreads=value)))))
            if "NativeRepositoryPulls(" in query:
                return ok(dict(data=dict(repository=dict(pullRequests=connection([pull(account)])))))
            if "NativePullComments(" in query:
                return ok(pull_data("comments", connection([comment()] + self.comments[account])))
            if "NativePullReviews(" in query:
                review = dict(id="PRR_fixture", author=dict(login="fixture-reviewer"), state="COMMENTED", body="Keep the keyboard behavior.", submittedAt=NOW, url="https://github.com/"+REPO+"/pull/7#pullrequestreview-1")
                return ok(pull_data("reviews", connection([review])))
            if "NativePullCommits(" in query:
                native_commit = dict(oid=SHA, message="Improve native navigation", committedDate=NOW, url="https://github.com/"+REPO+"/commit/"+SHA, author=dict(name="Fixture Author",user=dict(login="fixture-author")))
                return ok(pull_data("commits", connection([dict(commit=native_commit)])))
            if "NativePull(" in query:
                return ok(dict(data=dict(repository=dict(pullRequest=pull(account, variables["number"])))))
            return 422, dict(message="Unimplemented fixture GraphQL operation"), {}
        if route == "/users/fixture-org":
            return ok(dict(login="fixture-org", type="Organization"))
        if route.startswith("/users/") and route.endswith("/events"):
            return ok([dict(id="1",type="PullRequestEvent",repo=dict(name=REPO),created_at=NOW,payload=dict(action="opened",pull_request=dict(title="Fixture PR",html_url="https://github.com/"+REPO+"/pull/7")))])
        if route.startswith("/users/"):
            return ok(user(route.rsplit("/",1)[-1]))
        if route.endswith("/memberships/fixture-author"):
            return ok(dict(role="member"))
        if route.endswith("/members"):
            return ok([user()])
        if "/teams" in route:
            team=dict(slug="native-team",name="Native Team",description="Fixture team",members_count=1,repos_count=1,html_url="https://github.com/orgs/fixture-org/teams/native-team")
            return ok([team] if route.endswith("/teams") else team)
        if route == "/orgs/fixture-org/repos":
            return ok([repository(),repository("denied"),repository("limited")])
        if route.endswith("/languages"):
            return ok(dict(Rust=12000,TypeScript=2000))
        if route.endswith("/contributors"):
            return ok([dict(user(),contributions=12)])
        if route.endswith("/actions/runs"):
            return ok(dict(workflow_runs=[dict(name="Fixture native tests",status="completed",conclusion="success",head_branch="main",created_at=NOW,html_url="https://github.com/"+REPO+"/actions/runs/1")]))
        if route.endswith("/contents/.coderabbit.yaml"):
            return ok(dict(name=".coderabbit.yaml"))
        if route.endswith("/check-runs"):
            return ok(dict(check_runs=[dict(id=2,name="native-tests",status="completed",conclusion="success",app=dict(id=1,name="Fixture CI"),started_at=NOW,completed_at=NOW,html_url="https://github.com/"+REPO+"/actions/runs/1")]))
        if route.endswith("/statuses"):
            return ok([])
        if route.endswith("/timeline"):
            return ok([dict(id=1,event="review_requested",actor=dict(login="fixture-reviewer"),created_at=NOW)])
        if route.endswith("/files"):
            return ok([file_diff(),file_diff("assets/fixture.bin",None)])
        if method == "POST" and re.search(r"/issues/\d+/comments$",route):
            self.comments[account].append(comment("IC_new",body["body"],account))
            if body["body"] == "@coderabbitai full review":
                self.comments[account].append(comment("IC_bot", "## Walkthrough\nSynthetic completed review.", "coderabbitai[bot]"))
            return 201, dict(id=301), {}
        if method == "POST" and route.endswith("/requested_reviewers"):
            return 201, dict(id=7, requested_reviewers=[dict(login="copilot-pull-request-reviewer[bot]")]), {}
        if method == "POST" and route.endswith("/reviews"):
            return 201, dict(id=302,state="APPROVED",commit_id=body["commit_id"]), {}
        if route.endswith("/commits"):
            return ok([commit()])
        if "/commits/" in route:
            return ok(commit())
        if route.endswith("/issues"):
            return ok([issue()])
        if route.endswith("/issues/11/comments"):
            return ok([dict(id=1,user=dict(login="fixture-author"),body="Fixture follow-up comment",created_at=NOW,updated_at=NOW,html_url="https://github.com/"+REPO+"/issues/11#issuecomment-1")])
        if route.endswith("/issues/11"):
            return ok(issue())
        if route == "/repos/" + REPO:
            return ok(repository())
        return 404, dict(message="Unimplemented fixture endpoint"), {}
