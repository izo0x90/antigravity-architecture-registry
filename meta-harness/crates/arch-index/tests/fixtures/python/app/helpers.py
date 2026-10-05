class Base:
    def describe(self) -> str:
        return "base"


def gather(*items, **options) -> int:
    return len(items)


def clean(s: str) -> str:
    return s.strip()


def load():
    return " data "
