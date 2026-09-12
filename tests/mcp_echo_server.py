"""Minimal stdio MCP server used by tool-binding tests."""

from mcp.server.fastmcp import FastMCP

server = FastMCP("echo-service")


@server.tool()
def echo(text: str, times: int = 1) -> str:
    """Echo the given text N times."""
    return " ".join([text] * int(times))


if __name__ == "__main__":
    server.run()
