from helpers import clean, load, Base
from helpers import clean as tidy
import helpers as h


class Greeter(Base):
    def greet(self, name: str) -> str:
        return self.format(clean(name))

    def format(self, text: str, loud=False) -> str:
        raise NotImplementedError


def main():
    data = load()
    Greeter().greet(data)


def shout(text: str, loud=False) -> str:
    return tidy(text)


async def fetch(items):
    later = lambda: h.load()
    if False:
        tidy("never")
    for item in items:
        item.greet("x")
