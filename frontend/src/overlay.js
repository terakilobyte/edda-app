import "./app.css";
import { mount } from "svelte";
import Overlay from "./lib/Overlay.svelte";

const app = mount(Overlay, { target: document.getElementById("overlay") });

export default app;
