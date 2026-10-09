import { render } from "solid-js/web";

import "../../../rust/backbeat_app/src/styles.css";
import { initTheme } from "../../../rust/backbeat_app/src/lib/theme";
import "./styles.css";
import { App } from "./App";

initTheme();

render(() => <App />, document.getElementById("root")!);
