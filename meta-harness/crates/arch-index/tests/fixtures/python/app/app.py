from helpers import clean, Base


class Greeter(Base):
    def greet(self, name: str) -> str:
        return self.format(clean(name))

    def format(self, text: str, loud=False) -> str:
        raise NotImplementedError


def main():
    Greeter().greet("x")
