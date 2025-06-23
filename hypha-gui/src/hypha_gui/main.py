# ruff: noqa: E501
import asyncio
import functools
import tkinter as tk
import typing
import threading
from tkinter import ttk

# hypha_pyのスタブをインポート
# ruff: noqa: F401
import hypha_py

# asyncio用のキューを使用
# GUIスレッドとasyncioスレッド間で安全に通信するために使用
output_queue: asyncio.Queue[str] = asyncio.Queue()

# --- グローバル変数 ---
# これらはasyncioスレッドで管理される
hypha_instance: typing.Optional["hypha_py.Instance"] = None
hypha_connection: typing.Optional["hypha_py.Connection"] = None
close_handles: list["hypha_py.CloseHandle"] = []
running_task: typing.Optional[asyncio.Task] = None

# --- スレッド間で共有する状態 ---
app_running = True

# --- asyncioを別スレッドで実行するためのクラス ---
class AsyncioThread(threading.Thread):
    def __init__(self):
        super().__init__(daemon=True)
        self.loop = asyncio.new_event_loop()

    def run(self):
        asyncio.set_event_loop(self.loop)
        self.loop.run_forever()

    def stop(self):
        self.loop.call_soon_threadsafe(self.loop.stop)

    def create_task(self, coro):
        return asyncio.run_coroutine_threadsafe(coro, self.loop)

# --- asyncioスレッドで実行される非同期関数 ---
async def run_join_command(node_id_str: str):
    global hypha_instance, hypha_connection, running_task
    try:
        target_node_id = hypha_py.NodeId(node_id_str)
        secret = hypha_py.Secret()
        instance = await hypha_py.Instance.bind(secret)
        hypha_instance = instance

        connection = await instance.connect(instance, target_node_id)
        hypha_connection = connection

        await output_queue.put(f"Connected to {node_id_str}")
    except Exception as e:
        await output_queue.put(f"Error: {e}")
        await output_queue.put("STOP_GUI")
    finally:
        running_task = None


async def run_host_command():
    global hypha_instance, hypha_connection, running_task
    try:
        secret = hypha_py.Secret()
        instance = await hypha_py.Instance.bind(secret)
        hypha_instance = instance

        node_id = secret.node_id()
        await output_queue.put(f"Your Node ID:{node_id}")
        await output_queue.put("Waiting for connection...")

        connection = await instance.accept(instance)
        hypha_connection = connection

        await output_queue.put("Peer connected!")
    except Exception as e:
        await output_queue.put(f"Error: {e}")
        await output_queue.put("STOP_GUI")
    finally:
        running_task = None

# --- GUIスレッドで実行される関数 ---
def check_queue(root: tk.Tk, host_id_label: ttk.Label, status_label: ttk.Label, stop_func):
    """GUIスレッドからキューをチェックして画面を更新する."""
    try:
        while not output_queue.empty():
            message = output_queue.get_nowait()
            if message == "STOP_GUI":
                stop_func()
            elif message.startswith("Your Node ID:"):
                your_id = message.split(":", 1)[1].strip()
                host_id_label.config(text=your_id)
                status_label.config(text="Waiting for connection...")
            else:
                status_label.config(text=message)
    finally:
        if app_running:
            root.after(100, lambda: check_queue(root, host_id_label, status_label, stop_func))


def copy_id(root: tk.Tk, host_id_label: ttk.Label, status_label: ttk.Label):
    your_id = host_id_label.cget("text")
    if your_id:
        root.clipboard_clear()
        root.clipboard_append(your_id)
        status_label.config(text=f"ID: {your_id} is copied!")


def join(asyncio_thread: AsyncioThread, id_entry: ttk.Entry, host_button: ttk.Button, join_button: ttk.Button, status_label: ttk.Label):
    global running_task
    node_id_str = id_entry.get()
    if node_id_str and not running_task:
        status_label.config(text=f"Connecting to Node {node_id_str}...")
        running_task = asyncio_thread.create_task(run_join_command(node_id_str))
        host_button.config(state="disabled")
        join_button.config(state="disabled")


def host(asyncio_thread: AsyncioThread, host_button: ttk.Button, join_button: ttk.Button):
    global running_task
    if not running_task:
        running_task = asyncio_thread.create_task(run_host_command())
        host_button.config(state="disabled")
        join_button.config(state="disabled")


def stop(host_button: ttk.Button, join_button: ttk.Button, host_id_label: ttk.Label, status_label: ttk.Label):
    global running_task
    if running_task and not running_task.done():
        # asyncioスレッド内のタスクをキャンセル
        future = running_task
        future.cancel()
        running_task = None

    status_label.config(text="Stopped")
    host_id_label.config(text="")
    host_button.config(state="normal")
    join_button.config(state="normal")

def start():
    """アプリケーションを起動するための同期エントリポイント。"""
    global app_running

    # --- asyncioスレッドの準備と開始 ---
    asyncio_thread = AsyncioThread()
    asyncio_thread.start()

    # --- GUIの準備 ---
    root = tk.Tk()
    root.title("インターネット電話")
    frame = ttk.Frame(root, padding="10")
    frame.grid(row=0, column=0, sticky="nsew")

    # --- ウィジェットの作成 ---
    id_label = ttk.Label(frame, text="Node ID:")
    id_entry = ttk.Entry(frame, width=50)
    status_label = ttk.Label(frame, text="Waiting")
    host_id_label = ttk.Label(frame, text="", foreground="blue")
    join_button = ttk.Button(frame, text="Join")
    host_button = ttk.Button(frame, text="Host")
    copy_button = ttk.Button(frame, text="copy", width=10)
    stop_button = ttk.Button(frame, text="Stop")

    # --- コマンドへの部分適用 ---
    stop_func = functools.partial(stop, host_button, join_button, host_id_label, status_label)
    join_button["command"] = functools.partial(join, asyncio_thread, id_entry, host_button, join_button, status_label)
    host_button["command"] = functools.partial(host, asyncio_thread, host_button, join_button)
    copy_button["command"] = functools.partial(copy_id, root, host_id_label, status_label)
    stop_button["command"] = stop_func

    # --- ウィジェットの配置 ---
    id_label.grid(row=0, column=0, sticky=tk.W, padx=5, pady=5)
    id_entry.grid(row=0, column=1, columnspan=2, sticky="ew", padx=5, pady=5)
    join_button.grid(row=1, column=1, columnspan=2, pady=5, sticky="ew")
    ttk.Label(frame, text="Your ID:").grid(row=2, column=0, sticky=tk.W, padx=5, pady=5)
    host_id_label.grid(row=2, column=1, sticky="w", padx=5, pady=5)
    copy_button.grid(row=2, column=2, sticky="w", padx=5, pady=5)
    host_button.grid(row=3, column=1, columnspan=2, pady=5, sticky="ew")
    status_label.grid(row=4, column=0, columnspan=3, pady=10)
    stop_button.grid(row=5, column=1, columnspan=2, pady=10, sticky="ew")
    root.columnconfigure(0, weight=1)
    frame.columnconfigure(1, weight=1)

    # --- 終了処理 ---
    def on_closing():
        global app_running
        app_running = False
        stop_func()  # 実行中のタスクがあればキャンセル
        asyncio_thread.stop() # asyncioスレッドを停止
        root.destroy()

    root.protocol("WM_DELETE_WINDOW", on_closing)

    # --- GUIスレッドからキューのポーリングを開始 ---
    root.after(100, lambda: check_queue(root, host_id_label, status_label, stop_func))

    # --- Tkinterのメインループを開始 ---
    root.mainloop()

if __name__ == "__main__":
    start()