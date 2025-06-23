import tkinter as tk
from tkinter import ttk, scrolledtext
import subprocess
import threading
import queue
import hypha_py
import typing
import asyncio





output_queue = asyncio.Queue()

hypha_instance: typing .Optional[hypha_py.Instance] = None
hypha_connection: typing.Optional[hypha_py.Connection] = None
close_handles: typing.List[hypha_py.CloseHandle] = []
running_task: typing.Optional[asyncio.Task] = None

# --- GUIとasyncioを連携させるためのヘルパー ---
async def run_tk(root: tk.Tk):
    """TkinterのGUIを更新し続けるための非同期タスク"""
    try:
        while True:
            # GUIの状態を更新
            root.update()
            root.update_idletasks()
            # asyncioのイベントループに制御を少し戻す
            await asyncio.sleep(0.02)  # 20msごとに更新
    except tk.TclError:
        # ウィンドウが閉じられたときに発生するエラーを捕捉
        pass

# --- メインロジック ---

def copy_id(root, host_id_label, status_label):
    your_id = host_id_label.cget("text")
    if your_id:
        root.clipboard_clear()
        root.clipboard_append(your_id)
        status_label.config(text=f"ID: {your_id} is copied!")

def join(id_entry, host_button, join_button, status_label):
    """「Join」ボタンの処理。接続タスクを開始する"""
    global running_task
    node_id_str = id_entry.get()
    if node_id_str and not running_task:
        status_label.config(text=f"Connecting to Node {node_id_str}...")
        # 接続処理を非同期タスクとして開始
        running_task = asyncio.create_task(run_join_command(node_id_str))
        host_button.config(state="disabled")
        join_button.config(state="disabled")

async def run_join_command(node_id_str: str):
    """バックグラウンドで指定されたNodeに非同期で接続する"""
    global hypha_instance, hypha_connection, close_handles
    try:
        target_node_id = hypha_py.NodeId(node_id_str)
        secret = hypha_py.Secret()
        # awaitを使って非同期メソッドを呼び出す
        instance = await hypha_py.Instance.bind(secret)
        hypha_instance = instance
        # close_handles.append(instance.close_handle()) # Note: close_handleの仕様による

        # awaitを使って非同期メソッドを呼び出す
        connection = await instance.connect(instance, target_node_id)
        hypha_connection = connection
        # close_handles.append(connection.close_handle())

        await output_queue.put(f"Connected to {node_id_str}")
        
        # 音声ストリームを開始
        # play_stream = await connection.play()
        # record_stream = await connection.record()

    except Exception as e:
        await output_queue.put(f"Error: {e}")
        await output_queue.put("STOP") # エラー発生時に状態をリセット

def host(host_button, join_button):
    """「Host」ボタンの処理。ホストタスクを開始する"""
    global running_task
    if not running_task:
        # ホスト処理を非同期タスクとして開始
        running_task = asyncio.create_task(run_host_command())
        host_button.config(state="disabled")
        join_button.config(state="disabled")

async def run_host_command():
    """バックグラウンドでホストとして非同期で接続を待ち受ける"""
    global hypha_instance, hypha_connection, close_handles
    try:
        secret = hypha_py.Secret()
        # awaitを使って非同期メソッドを呼び出す
        instance = await hypha_py.Instance.bind(secret)
        hypha_instance = instance
        # close_handles.append(instance.close_handle())

        node_id = secret.node_id()
        await output_queue.put(f"Your Node ID:{node_id}")
        await output_queue.put("Waiting for connection...")
        
        # awaitを使って非同期で接続を待ち受ける
        connection = await instance.accept(instance)
        hypha_connection = connection
        # close_handles.append(connection.close_handle())
        
        await output_queue.put("Peer connected!")

        # 音声ストリームを開始
        # play_stream = await connection.play()
        # record_stream = await connection.record()

    except Exception as e:
        await output_queue.put(f"Error: {e}")
        await output_queue.put("STOP") # エラー発生時に状態をリセット

async def check_queue(host_id_label, status_label, stop_func):
    """キューを定期的にチェックしてGUIを更新する非同期タスク"""
    while True:
        message = await output_queue.get()
        if message == "STOP":
            stop_func()
        elif isinstance(message, str) and message.startswith("Your Node ID:"):
            your_id = message.split(":")[1].strip()
            host_id_label.config(text=your_id)
            status_label.config(text="Waiting for connection...")
        else:
            status_label.config(text=message)
        # キューの処理が完了したことを通知
        output_queue.task_done()

def stop(host_button, join_button, host_id_label, status_label):
    """「Stop」ボタンの処理"""
    global running_task, hypha_instance, hypha_connection, close_handles
    
    if running_task:
        running_task.cancel() # 実行中のタスクをキャンセル
        running_task = None
    
    # close()メソッド自体が非同期の可能性もあるが、ここでは同期的に呼び出す
    for handle in reversed(close_handles):
        try:
            handle.close()
        except Exception as e:
            print(f"Error closing handle: {e}")

    hypha_instance = None
    hypha_connection = None
    close_handles = []

    status_label.config(text="Stopped")
    host_id_label.config(text="")
    host_button.config(state="normal")
    join_button.config(state="normal")


async def main():
    # --- ウィンドウの作成 ---
    root = tk.Tk()
    root.title("インターネット電話 (asyncio)")
    frame = ttk.Frame(root, padding="10")
    frame.grid(row=0, column=0, sticky="nsew")

    # --- ウィジェットの作成 ---
    id_label = ttk.Label(frame, text="Node ID:")
    id_entry = ttk.Entry(frame, width=50)
    status_label = ttk.Label(frame, text="Waiting")
    host_id_label = ttk.Label(frame, text="", foreground="blue")
    
    # --- stop関数を部分適用で作成 ---
    # stop関数にウィジェットを渡すため、lambdaやfunctools.partialを使う
    stop_func = lambda: stop(host_button, join_button, host_id_label, status_label)

    # --- ウィジェットの配置とコマンド設定 ---
    join_button = ttk.Button(frame, text="Join", command=lambda: join(id_entry, host_button, join_button, status_label))
    host_button = ttk.Button(frame, text="Host", command=lambda: host(host_button, join_button))
    copy_button = ttk.Button(frame, text="copy", command=lambda: copy_id(root, host_id_label, status_label), width=10)
    stop_button = ttk.Button(frame, text="Stop", command=stop_func)
    
    id_label.grid(row=0, column=0, sticky=tk.W, padx=5, pady=5)
    id_entry.grid(row=0, column=1, columnspan=2, sticky="ew", padx=5, pady=5)
    join_button.grid(row=1, column=1, columnspan=2, pady=5, sticky="ew")
    ttk.Label(frame, text="Your ID:").grid(row=2, column=0, sticky=tk.W, padx=5, pady=5)
    host_id_label.grid(row=2, column=1, columnspan=1, sticky="w", padx=5, pady=5)
    copy_button.grid(row=2, column=2, sticky="w", padx=5, pady=5)
    host_button.grid(row=3, column=1, columnspan=2, pady=5, sticky="ew")
    status_label.grid(row=4, column=0, columnspan=3, pady=10)
    stop_button.grid(row=5, column=1, columnspan=2, pady=10, sticky="ew")
    
    root.columnconfigure(0, weight=1)
    frame.columnconfigure(1, weight=1)

    # --- asyncioタスクの開始 ---
    tk_task = asyncio.create_task(run_tk(root))
    queue_task = asyncio.create_task(check_queue(host_id_label, status_label, stop_func))

    def on_closing():
        # タスクをキャンセルしてからウィンドウを閉じる
        tk_task.cancel()
        queue_task.cancel()
        if running_task:
            running_task.cancel()
        # asyncioループを止めるためにroot.quit()を使う
        root.quit()

    root.protocol("WM_DELETE_WINDOW", on_closing)

    # Tkinterのタスクが完了するまで（ウィンドウが閉じられるまで）待機
    await tk_task


if __name__ == '__main__':
    # asyncioのイベントループを開始
    try:
        asyncio.run(main())
    except asyncio.CancelledError:
        # プログラム終了時のキャンセルエラーは無視
        pass
