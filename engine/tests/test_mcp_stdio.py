import io
import json

from sakura import mcp_stdio


def echo_tools():
    spec = {"name": "echo", "description": "Echo the text.", "inputSchema": {"type": "object"}}
    return [mcp_stdio.Tool(spec, lambda arguments: (str(arguments.get("text", "")), arguments.get("fail") is True))]


def converse(*messages, tools=None, raw=None):
    stdin = io.StringIO(raw if raw is not None else "".join(json.dumps(message) + "\n" for message in messages))
    stdout = io.StringIO()
    mcp_stdio.serve("demo", tools or echo_tools(), stdin, stdout)
    return [json.loads(line) for line in stdout.getvalue().splitlines()]


def request(identifier, method, **params):
    return {"jsonrpc": "2.0", "id": identifier, "method": method, "params": params}


def test_McpStdio_Notification_NoAnswer():
    answers = converse({"jsonrpc": "2.0", "method": "notifications/initialized"}, request(1, "ping"))
    assert answers == [{"jsonrpc": "2.0", "id": 1, "result": {}}]


def test_McpStdio_Initialize_EchoesProtocolAndNamesServer():
    answer = converse(request(1, "initialize", protocolVersion="2025-06-18"))[0]["result"]
    assert answer["protocolVersion"] == "2025-06-18" and answer["serverInfo"]["name"] == "demo"
    assert answer["capabilities"] == {"tools": {}}


def test_McpStdio_ToolsList_ReturnsTheSpecs():
    answer = converse(request(1, "tools/list"))[0]["result"]
    assert [tool["name"] for tool in answer["tools"]] == ["echo"]


def test_McpStdio_ToolCall_TextAndIsError():
    answers = converse(request(1, "tools/call", name="echo", arguments={"text": "hi"}),
                       request(2, "tools/call", name="echo", arguments={"text": "no", "fail": True}))
    assert answers[0]["result"] == {"content": [{"type": "text", "text": "hi"}], "isError": False}
    assert answers[1]["result"]["isError"] is True


def test_McpStdio_UnknownToolOrMethod_JsonRpcError():
    answers = converse(request(1, "tools/call", name="nope", arguments={}), request(2, "frobnicate"),
                       request(3, "tools/call", name="echo", arguments=["not", "an object"]))
    assert [answer["error"]["code"] for answer in answers] == [-32602, -32601, -32602]


def test_McpStdio_InvalidJson_ParseErrorAndNextLineServed():
    answers = converse(raw="{broken\n" + json.dumps(request(1, "ping")) + "\n")
    assert answers[0]["error"]["code"] == -32700 and answers[0]["id"] is None
    assert answers[1]["result"] == {}


def test_McpStdio_ToolRaises_AnswersIsErrorNamingTheClassAndKeepsServing():
    def broken(arguments):
        raise RuntimeError("boom")

    spec = {"name": "broken", "description": "x", "inputSchema": {"type": "object"}}
    answers = converse(request(1, "tools/call", name="broken", arguments={}), request(2, "ping"),
                       tools=[mcp_stdio.Tool(spec, broken)])
    assert answers[0]["result"]["isError"] is True and "RuntimeError" in answers[0]["result"]["content"][0]["text"]
    assert answers[1]["result"] == {}


def test_McpStdio_LineOverLimit_ErrorAndSkippedNextLineServed():
    huge = json.dumps(request(1, "tools/call", name="echo", arguments={"text": "x" * mcp_stdio.MAX_LINE}))
    answers = converse(raw=huge + "\n" + json.dumps(request(2, "ping")) + "\n")
    assert answers[0]["error"]["code"] == -32600 and answers[0]["id"] is None
    assert answers[1] == {"jsonrpc": "2.0", "id": 2, "result": {}}
    assert len(answers) == 2


def test_McpStdio_LineExactlyAtLimit_Served():
    line = json.dumps(request(1, "ping"))
    padded = line[:-1] + " " * (mcp_stdio.MAX_LINE - len(line)) + "}"
    assert len(padded) == mcp_stdio.MAX_LINE
    assert converse(raw=padded + "\n")[0]["result"] == {}
