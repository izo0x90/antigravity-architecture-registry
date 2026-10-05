import { clean } from "./helpers";

export function View(props: { name: string }) {
  return <div>{clean(props.name)}</div>;
}
