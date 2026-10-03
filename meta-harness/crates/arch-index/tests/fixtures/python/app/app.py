from helpers import clean, load, Base


class Greeter(Base):
    def greet(self, name: str) -> str:
        return self.format(clean(name))

    def format(self, text: str, loud=False) -> str:
        raise NotImplementedError


def main():
    data = load()
    Greeter().greet(data)
