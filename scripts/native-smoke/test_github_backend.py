import importlib.util
import json
from pathlib import Path
import socket
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location("github_backend", Path(__file__).with_name("github-backend.py"))
backend = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backend)


class GitHubBackendTests(unittest.TestCase):
    def test_explicit_synthetic_account_is_scoped_to_its_owned_server(self):
        with backend.GitHubBackend(accounts=("synthetic-account",)) as server:
            self.assertEqual(self.request(server, "/users/fixture-author", account="synthetic-account")[0], 200)
            self.assertEqual(self.request(server, "/users/fixture-author", account="fixture-one")[0], 401)
            self.assertEqual(self.request(server, "/users/fixture-author", account="real-account")[0], 401)
    def test_loopback_server_serves_real_requests_without_hostname_discovery(self):
        with patch.object(socket, "getfqdn", side_effect=AssertionError("The numeric loopback fixture must not discover local hostnames")):
            with backend.GitHubBackend() as server:
                self.assertEqual(server.server.server_address[0], "127.0.0.1")
                self.assertEqual(server.server.server_name, "127.0.0.1")
                self.assertGreater(server.server.server_port, 0)
                status, response, _ = self.request(server, "/users/fixture-author")
                self.assertEqual((status, response["login"]), (200, "fixture-author"))
                self.assertEqual(len(server.requests), 1)

    def request(self, server, path, body=None, account="fixture-one"):
        request = Request(server.url.rstrip("/") + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Authorization": "Bearer native-fixture-" + account, "Content-Type": "application/json"})
        try:
            with urlopen(request, timeout=2) as response:
                return response.status, json.load(response), response.headers
        except HTTPError as error:
            return error.code, json.load(error), error.headers

    def test_real_http_authentication_and_account_payloads_are_distinct(self):
        with backend.GitHubBackend() as server:
            body = dict(query="query NativePullRequests($query:String!){search{nodes{number}}}", variables=dict(query="is:pr author:fixture-one is:open"))
            first = self.request(server, "/graphql", body)[1]["data"]["search"]
            second = self.request(server, "/graphql", body, "fixture-two")[1]["data"]["search"]
            self.assertIn("First private", first["nodes"][0]["title"])
            self.assertIn("Second private", second["nodes"][0]["title"])
            self.assertTrue(first["pageInfo"]["hasNextPage"])
            self.assertEqual(self.request(server, "/users/fixture-org", account="real-account")[0], 401)
            self.assertNotIn("Authorization", json.dumps(server.requests))

    def test_confirmed_fixture_write_is_observable_once_and_isolated_by_account(self):
        with backend.GitHubBackend() as server:
            status, response, _ = self.request(server, "/repos/fixture-org/native-demo/issues/7/comments", dict(body="Installed native fixture comment"))
            self.assertEqual((status, response["id"]), (201, 301))
            self.assertEqual(len(server.comments["fixture-one"]), 1)
            self.assertEqual(server.comments["fixture-two"], [])
            self.assertEqual(len([call for call in server.requests if call["method"] == "POST"]), 1)

    def test_empty_search_and_viewer_fallback_are_independent_sources(self):
        with backend.GitHubBackend() as server:
            server.scenario = "search-degraded"
            body = dict(query="query NativePullRequests($query:String!){search{nodes{number}}}", variables=dict(query="is:pr author:fixture-one is:open"))
            self.assertEqual(self.request(server, "/graphql", body)[1]["data"]["search"]["nodes"], [])
            fallback = dict(query="query NativeViewerPulls($reviewer:String!){viewer{login}}", variables=dict(reviewer="fixture-one"))
            viewer = self.request(server, "/graphql", fallback)[1]["data"]["viewer"]
            self.assertEqual(viewer["login"], "fixture-one")
            self.assertEqual(viewer["pullRequests"]["nodes"][0]["author"]["login"], "fixture-one")
            server.scenario = "empty"
            self.assertEqual(self.request(server, "/graphql", fallback)[1]["data"]["viewer"]["pullRequests"]["nodes"], [])

    def test_denial_rate_limit_and_unknown_endpoints_fail_explicitly(self):
        with backend.GitHubBackend() as server:
            self.assertEqual(self.request(server, "/repos/fixture-org/denied")[0], 403)
            status, _, headers = self.request(server, "/repos/fixture-org/limited")
            self.assertEqual((status, headers["Retry-After"]), (429, "60"))
            self.assertEqual(self.request(server, "/unimplemented")[0], 404)
            server.scenario = "auth"
            self.assertEqual(self.request(server, "/users/fixture-org", account="fixture-two")[0], 401)


if __name__ == "__main__":
    unittest.main()
