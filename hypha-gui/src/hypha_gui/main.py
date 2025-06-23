import tkinter as tk
from tkinter import ttk
import subprocess
import threading
import queue

process = None
output_queue = queue.Queue()


def copy_id():
    # host_id_labelから現在のIDテキストを取得
    your_id = host_id_label.cget("text")
    if your_id:
        root.clipboard_clear()  # クリップボードをクリア
        root.clipboard_append(your_id)  # クリップボードにIDを追加
        status_label.config(text=f"ID: {your_id} is copied!")

def join():
    global process
    node_id = id_entry.get()
    if node_id and not process:
        process = subprocess.Popen(["./../target/debug/hypha-cli","join", node_id])
        status_label.config(text=f"connecting to Node {node_id}")


def host():
    """「Host」ボタンの処理。コマンド実行を別スレッドで開始する"""
    global process
    if not process:
        # コマンド実行を別スレッドで開始
        # daemon=Trueにすると、メインプログラム終了時にスレッドも終了する
        thread = threading.Thread(target=run_host_command, daemon=True)
        thread.start()
        # 実行中はボタンを無効化する
        host_button.config(state="disabled")
        join_button.config(state="disabled")


def run_host_command():
    """バックグラウンドスレッドでホストコマンドを実行し、出力をキューに入れる"""
    global process
    host_command = ["./../target/debug/hypha-cli","host"]

    try:
        process = subprocess.Popen(
            host_command,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, # エラー出力も標準出力にまとめる
            text=True,
            encoding='utf-8',
            bufsize=1 # 行単位のバッファリングを強制
        )
        
        # プロセスの出力を1行ずつ読み取り、キューに入れる
        for line in iter(process.stdout.readline, ''):
            if line:
                output_queue.put(line.strip()) # strip()で不要な改行を削除
        
        process.stdout.close()
        process.wait()

    except FileNotFoundError:
        output_queue.put("エラー: 指定されたコマンドが見つかりません。")
    except Exception as e:
        output_queue.put(f"エラー: {e}")
    finally:
        # 処理完了後にキューに目印を入れる
        output_queue.put(None)


def check_queue():
    """キューを定期的にチェックしてGUIを更新する"""
    try:
        while True:
            # キューからノンブロッキングでデータを取得
            message = output_queue.get_nowait()
            
            if message is None: # 処理完了の目印を受け取った場合
                stop() # プロセスが正常終了した場合もstop()を呼んで状態をリセット
                break
            
            # メッセージの内容に応じて表示を分ける
            if message.startswith("Your Node ID:"):
                # "Your ID :" ラベルを更新
                your_id = message.split(":")[1].strip()
                host_id_label.config(text=your_id)
            else:
                # "status_label" を更新
                status_label.config(text=message)

    except queue.Empty: # キューが空の場合は何もしない
        pass
    finally:
        # 100ミリ秒後に再度この関数を実行する
        root.after(100, check_queue)

def stop():
    """「Stop」ボタンの処理"""
    global process
    if process:
        process.terminate() # プロセスを終了
        process = None
    status_label.config(text="Stopped")
    host_id_label.config(text="") # ID表示をリセット
    # ボタンを再度有効化する
    host_button.config(state="normal")
    join_button.config(state="normal")


# --- ウィンドウの作成 ---
root = tk.Tk()
root.title("インターネット電話")

frame = ttk.Frame(root, padding="10")
frame.grid(row=0, column=0, sticky="nsew")

# --- ウィジェットの配置 ---
id_label = ttk.Label(frame, text="Node ID:")
id_label.grid(row=0, column=0, sticky=tk.W, padx=5, pady=5)
id_entry = ttk.Entry(frame, width=50)
id_entry.grid(row=0, column=1, columnspan=2, sticky="ew", padx=5, pady=5)

join_button = ttk.Button(frame, text="Join", command=join)
join_button.grid(row=1, column=1, columnspan=2, pady=5, sticky="ew")

host_id_title_label = ttk.Label(frame, text="Your ID:")
host_id_title_label.grid(row=2, column=0, sticky=tk.W, padx=5, pady=5)
# ID表示用のラベルを分ける
host_id_label = ttk.Label(frame, text="", foreground="blue")
host_id_label.grid(row=2, column=1, columnspan=1, sticky="w", padx=5, pady=5)

copy_button = ttk.Button(frame, text="copy", command=copy_id, width=10)
copy_button.grid(row=2, column=2, sticky="w", padx=5, pady=5)

host_button = ttk.Button(frame, text="Host", command=host)
host_button.grid(row=3, column=1, columnspan=2, pady=5, sticky="ew")

status_label = ttk.Label(frame, text="Waiting")
status_label.grid(row=4, column=0, columnspan=2, pady=10)

stop_button = ttk.Button(frame, text="Stop", command=stop)
stop_button.grid(row=5, column=1, columnspan=2, pady=10, sticky="ew")

# ウィンドウのリサイズに対応
root.columnconfigure(0, weight=1)
frame.columnconfigure(1, weight=1)

# キューの監視を開始
check_queue()

root.mainloop()


