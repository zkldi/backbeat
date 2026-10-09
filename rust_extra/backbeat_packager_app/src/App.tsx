import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { createStore } from "solid-js/store";

// yuck, don't care though
import buttons from "../../../rust/backbeat_app/src/components/primitives/Button.module.css";
import shell from "../../../rust/backbeat_app/src/components/window/Shell.module.css";
import { version } from "../src-tauri/tauri.conf.json";

interface Packaged {
	charts: number;
	missing: string[];
	output: string;
}
type Mode = "archive" | "store";
interface Result {
	error?: string;
	path: string;
	result?: Packaged;
	status: "Failed" | "Imported" | "Running" | "Saved";
}
interface Discovery {
	errors: string[];
	paths: string[];
}

const extensions = ["sm", "ssc", "dwi", "bms", "bme", "bml", "pms", "bmson", "ksh", "kson"];
const basename = (path: string) => path.split(/[\\/]/u).pop() ?? path;

export function App() {
	const [tab, setTab] = createSignal("about");
	const [pending, setPending] = createSignal<string[]>([]);
	const [results, setResults] = createStore<Result[]>([]);
	const [storePath, setStorePath] = createSignal("");
	const [storeError, setStoreError] = createSignal("");
	const [messages, setMessages] = createSignal<string[]>([]);
	const [busy, setBusy] = createSignal(false);
	const [scanning, setScanning] = createSignal(false);
	const [hovering, setHovering] = createSignal(false);
	const [stopping, setStopping] = createSignal(false);
	const [limit, setLimit] = createSignal(100);
	const [progress, setProgress] = createSignal({ done: 0, total: 0 });
	let disposed = false;
	let unlisten: (() => void) | undefined;
	const locked = () => busy() || scanning();
	const report = (error: unknown) => setMessages((previous) => [...previous, String(error)]);
	async function openFolder(path: string) {
		try {
			await invoke("open_result_folder", { path });
		} catch (error) {
			report(error);
		}
	}

	async function loadStore() {
		setStoreError("");
		try {
			setStorePath(await invoke<string>("store_destination"));
		} catch (error) {
			setStoreError(String(error));
		}
	}

	async function addPaths(paths: string[]) {
		if (locked()) {
			return;
		}
		setScanning(true);
		setTab("package");
		setMessages([]);
		try {
			const found = await invoke<Discovery>("discover", { paths });
			await invoke("reset_batch");
			setPending(found.paths);
			setMessages(found.errors);
			if (!found.paths.length) {
				report("No supported charts found.");
			}
		} catch (error) {
			report(error);
		} finally {
			setScanning(false);
		}
	}

	async function browse() {
		try {
			const choice = await open({ directory: true, multiple: true, title: "Choose folders" });
			if (choice) {
				await addPaths(Array.isArray(choice) ? choice : [choice]);
			}
		} catch (error) {
			report(error);
		}
	}

	async function start(mode: Mode) {
		if (locked() || !pending().length) {
			return;
		}
		setBusy(true);
		setStopping(false);
		try {
			let destination = storePath();
			if (mode === "archive") {
				const choice = await open({ directory: true, title: "Save .bbzip files to" });
				if (typeof choice !== "string") {
					return;
				}
				destination = choice;
			}
			if (!destination) {
				return;
			}
			const paths = pending();
			setProgress({ done: 0, total: paths.length });
			setTab("results");
			const concurrency = await invoke<number>("packaging_concurrency");
			let next = 0;
			let processed = 0;
			async function worker() {
				while (!stopping() && next < paths.length) {
					const path = paths[next++];
					const index = results.length;
					setResults(index, { path, status: "Running" });
					try {
						const result = await invoke<Packaged>(
							mode === "archive" ? "package_chart" : "import_chart",
							mode === "archive"
								? { path, outputDir: destination }
								: { path, storePath: destination },
						);
						setResults(index, {
							status: mode === "archive" ? "Saved" : "Imported",
							result,
						});
					} catch (error) {
						setResults(index, { status: "Failed", error: String(error) });
					}
					processed += 1;
					setProgress({ done: processed, total: paths.length });
				}
			}
			await Promise.all(Array.from({ length: Math.min(concurrency, paths.length) }, worker));
			setPending(paths.slice(next));
		} catch (error) {
			report(error);
		} finally {
			setBusy(false);
			setStopping(false);
		}
	}

	onMount(() => {
		if (!isTauri()) {
			return;
		}
		void loadStore();
		void getCurrentWebview()
			.onDragDropEvent(({ payload }) => {
				setHovering(payload.type === "enter" || payload.type === "over");
				if (payload.type === "drop") {
					void addPaths(payload.paths);
				}
			})
			.then((stop) => {
				if (disposed) {
					stop();
				} else {
					unlisten = stop;
				}
			})
			.catch(report);
	});
	onCleanup(() => {
		disposed = true;
		unlisten?.();
	});

	return (
		<div class="packager-shell">
			<header class={shell.header}>
				<h1 class={shell.headerTitle}>zk's Backbeat Packager</h1>
			</header>
			<nav aria-label="Views">
				<button
					aria-current={tab() === "about" ? "page" : undefined}
					onClick={() => setTab("about")}
				>
					About
				</button>
				<button
					aria-current={tab() === "package" ? "page" : undefined}
					onClick={() => setTab("package")}
				>
					Package
				</button>
				<button
					aria-current={tab() === "results" ? "page" : undefined}
					onClick={() => setTab("results")}
				>
					Results ({results.length})
				</button>
			</nav>
			<main>
				<Show when={tab() === "package"}>
					<section class="package-view">
						<button
							class={`dropzone ${hovering() && !locked() ? "hovering" : ""}`}
							disabled={locked()}
							onClick={() => void browse()}
						>
							<strong>
								{scanning()
									? "Scanning folders"
									: pending().length
										? `${pending().length} charts ready`
										: "Drop charts or folders"}
							</strong>
						</button>
						<div class="destinations">
							<button
								class={buttons.button}
								data-variant="base"
								disabled={locked() || !pending().length}
								onClick={() => void start("archive")}
							>
								Save them as <code>.bbzip</code> files I can share
							</button>
							<button
								class={buttons.button}
								data-variant="base"
								disabled={locked() || !pending().length || !storePath()}
								onClick={() => void start("store")}
							>
								Import them into your store
							</button>
						</div>
						<Show when={storePath()}>
							<p class="store-path">Store: {storePath()}</p>
						</Show>
						<Show when={storeError()}>
							<p class="notice">
								{storeError()}{" "}
								<button
									class={buttons.button}
									data-variant="base"
									disabled={locked()}
									onClick={() => void loadStore()}
								>
									Reload store
								</button>
							</p>
						</Show>
					</section>
				</Show>
				<Show when={tab() === "results"}>
					<Show when={progress().total > 0}>
						<div class="progress">
							<span>
								{progress().done} of {progress().total} processed
							</span>
							<Show when={busy()}>
								<button
									class={buttons.button}
									data-variant="base"
									disabled={stopping()}
									onClick={() => setStopping(true)}
								>
									{stopping()
										? "Waiting for running files"
										: "Stop after running files"}
								</button>
							</Show>
						</div>
						<progress
							aria-label="Batch progress"
							max={progress().total}
							value={progress().done}
						/>
					</Show>
					<Show when={!results.length}>
						<p class="empty">You haven't packaged anything yet</p>
					</Show>
					<ul class="results">
						<For each={results.slice(-limit()).reverse()}>
							{(entry) => (
								<li class="result-card">
									<div class="result-heading">
										<strong>{basename(entry.path)}</strong>
										<span class="result-status" data-status={entry.status}>
											{entry.status}
										</span>
									</div>
									<p class="path" title={entry.path}>Source: {entry.path}</p>
									<Show when={entry.result}>
										{(result) => (
											<>
												<p class="result-count">
													{result().charts} {result().charts === 1 ? "chart" : "charts"}
												</p>
												<p class="path">
													{entry.status === "Imported"
														? "Store"
														: "Output"}
													: {result().output}
												</p>
												<button
													class={`${buttons.button} result-open`}
													data-variant="base"
													onClick={() => void openFolder(result().output)}
													title={result().output}
												>
													{entry.status === "Imported" ? "Open store folder" : "Open containing folder"}
												</button>
												<Show when={result().missing.length}>
													<details class="result-missing">
														<summary>{result().missing.length} missing {result().missing.length === 1 ? "asset" : "assets"}</summary>
														<ul><For each={result().missing}>{(path) => <li>{path}</li>}</For></ul>
													</details>
												</Show>
											</>
										)}
									</Show>
									<Show when={entry.error}>
										<p class="notice">{entry.error}</p>
										<button class={`${buttons.button} result-open`} data-variant="base" onClick={() => void openFolder(entry.path)}>Open source folder</button>
									</Show>
								</li>
							)}
						</For>
					</ul>
					<Show when={results.length > limit()}>
						<button
							class={buttons.button}
							data-variant="base"
							onClick={() => setLimit(limit() + 100)}
						>
							Show earlier results
						</button>
					</Show>
				</Show>
				<Show when={tab() === "about"}>
					<section class="about">
						<h1>zk's Backbeat Packager</h1>
						<p>
							Convert rhythm game charts into <code>.bbzip</code> files that can be sent around and uploaded to sites, or convert them and put them straight into your Backbeat store.
						</p>
						<p>
							This isn't a "canonical" or official Backbeat packager. There isn't one. <b>Backbeat doesn't care about or understand <code>.bms</code> or <code>.dtx</code> files</b>. However, someone somewhere has to turn <code>.bms</code> files into <code>.bb</code> files that backbeat understands.
						</p>
						<p>
							This is my personal packager, and it supports the things I personally wrote support for.
						</p>
						<p>
							Click '
							<a
								href="#package"
								onClick={(event) => {
									event.preventDefault();
									setTab("package");
								}}
							>
								Package
							</a>
							' at the top of this screen to get started.
						</p>
						<hr />
						<h2>Supported file extensions</h2>
						<p>Packaging requires game-specific logic, and this is <b>not</b> part of like, the "core" distribution of backbeat. I don't want to be maintaining rhythm game parsers for the rest of my life, as new features come in.</p>
						<p>Writing your own packager is easy. If you need a custom one, go write it (or ask your agent to, lol!)</p>
						<p>This packager comes with support for these file formats:</p>
						<p class="extensions">
							{extensions.map((extension) => `.${extension}`).join(", ")}
						</p>
					</section>
				</Show>
				<For each={messages()}>{(message) => <p class="notice">{message}</p>}</For>
			</main>
			<footer class="packager-footer">zk's Backbeat Packager v{version}</footer>
		</div>
	);
}
