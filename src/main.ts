import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { inTauri, windowKind } from "./lib/api";

const kind = windowKind();
const root = document.documentElement;
root.classList.add(`win-${kind}`);
if (!inTauri) root.classList.add("mock");

export default mount(App, { target: document.getElementById("app")!, props: { kind } });
