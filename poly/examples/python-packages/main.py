import click
import requests
from rich.console import Console
from rich.table import Table


@click.command()
@click.option("--name", default="Poly", show_default=True)
def main(name: str) -> None:
    request = requests.Request(
        "GET",
        "https://example.com/packages",
        params={"runtime": name.lower()},
    ).prepare()

    table = Table(title="Embedded Python packages")
    table.add_column("Package")
    table.add_column("Used for")
    table.add_row("click", "command-line options")
    table.add_row("requests", request.url)
    table.add_row("rich", "this table")
    Console().print(table)


if __name__ == "__main__":
    main()
