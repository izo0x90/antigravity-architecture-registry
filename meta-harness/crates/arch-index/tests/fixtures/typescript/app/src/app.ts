import { Base, Formatter, clean, load } from "./helpers";

export class Greeter extends Base implements Formatter {
  greet(name: string): string {
    return this.format(clean(name));
  }

  format(text: string): string {
    throw new Error("TODO: format");
  }
}

export function main(): void {
  const raw = load();
  new Greeter().greet(raw);
  show(raw);
}

function show(value: string): void {
  console.log(value);
}
