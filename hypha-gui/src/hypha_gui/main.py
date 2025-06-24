import tkinter as tk
from tkinter import ttk
import subprocess
import threading
import queue

process = None
output_queue = queue.Queue()

# --- 機能関数 ---

def copy_id():
    """Your IDをクリップボードにコピーする"""
    your_id = host_id_label.cget("text")
    if your_id:
        root.clipboard_clear()
        root.clipboard_append(your_id)
        status_label.config(text=f"ID: {your_id} is copied!")

def start_host():
    """「Host」ボタンの処理"""
    command = ["./../target/debug/hypha-cli", "host"]
    run_command_in_thread(command)

def start_join():
    """「Join」ボタンの処理"""
    node_id = id_entry.get()
    if node_id:
        command = ["./../target/debug/hypha-cli", "join", node_id]
        run_command_in_thread(command)
    else:
        status_label.config(text="Please enter a Node ID to join.")

def run_command_in_thread(command):
    """コマンド実行を別スレッドで開始する"""
    global process
    if not process:
        # 実行中はボタンや入力欄を無効化
        host_button.config(state="disabled")
        join_button.config(state="disabled")
        id_entry.config(state="disabled")
        status_label.config(text=f"Starting: {' '.join(command)}")

        # コマンド実行を別スレッドで開始
        thread = threading.Thread(target=run_command, args=(command,), daemon=True)
        thread.start()

def run_command(command):
    """バックグラウンドスレッドでコマンドを実行し、出力をキューに入れる"""
    global process
    try:
        process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,    # 標準入力をパイプで受け取る
            stdout=subprocess.PIPE,   # 標準出力をパイプで受け取る
            stderr=subprocess.STDOUT, # 標準エラー出力を標準出力にまとめる
            text=True,
            encoding='utf-8',
            bufsize=1                 # 行単位のバッファリング
        )
        
        # プロセスの出力を1行ずつ読み取り、キューに入れる
        for line in iter(process.stdout.readline, ''):
            if line:
                output_queue.put(line.strip())
        
        process.stdout.close()
        process.wait()

    except FileNotFoundError:
        output_queue.put("Error: The specified command was not found.")
    except Exception as e:
        output_queue.put(f"Error: {e}")
    finally:
        output_queue.put(None) # 処理完了の目印

def check_queue():
    """キューを定期的にチェックしてGUIを更新する"""
    try:
        while True:
            message = output_queue.get_nowait()
            
            if message is None: # 処理完了の目印
                stop()
                break
            
            if message.startswith("Your Node ID:"):
                your_id = message.split(":")[1].strip()
                host_id_label.config(text=your_id)
            else:
                status_label.config(text=message)

            # 接続が確立したことを示すメッセージを検知して入力欄を有効化
            # (※このメッセージは `hypha-cli` の実際の出力に合わせて調整してください)
            if "Let's talk" in message or "Listening for incoming connections" in message:
                message_entry.config(state="normal")
                send_button.config(state="normal")
                status_label.config(text="Connected! You can now send messages.")

    except queue.Empty:
        pass
    finally:
        root.after(100, check_queue)

def send_message(event=None):
    """メッセージ入力欄の内容をサブプロセスの標準入力に送信する"""
    if process and process.poll() is None:
        message = message_entry.get()
        if message:
            try:
                # text=Trueモードなので文字列の末尾に改行を追加して書き込む
                process.stdin.write(message + '\n')
                process.stdin.flush()
                message_entry.delete(0, tk.END) # 送信後に入力欄をクリア
            except (IOError, BrokenPipeError) as e:
                status_label.config(text=f"Failed to send message: {e}")

def stop():
    """「Stop」ボタンの処理。プロセスを終了し、UIをリセットする"""
    global process
    if process:
        if process.stdin:
            process.stdin.close()
        process.terminate()
        try:
            # 終了を待つ（タイムアウト付き）
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill() # 強制終了
        process = None

    # UIの状態をリセット
    status_label.config(text="Stopped")
    host_id_label.config(text="")
    host_button.config(state="normal")
    join_button.config(state="normal")
    id_entry.config(state="normal")
    
    # メッセージ入力欄もリセット
    message_entry.config(state="disabled")
    message_entry.delete(0, tk.END)
    send_button.config(state="disabled")

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

join_button = ttk.Button(frame, text="Join", command=start_join)
join_button.grid(row=1, column=1, columnspan=2, pady=5, sticky="ew")

host_id_title_label = ttk.Label(frame, text="Your ID:")
host_id_title_label.grid(row=2, column=0, sticky=tk.W, padx=5, pady=5)
host_id_label = ttk.Label(frame, text="", foreground="blue")
host_id_label.grid(row=2, column=1, columnspan=1, sticky="w", padx=5, pady=5)

copy_button = ttk.Button(frame, text="copy", command=copy_id, width=10)
copy_button.grid(row=2, column=2, sticky="w", padx=5, pady=5)

host_button = ttk.Button(frame, text="Host", command=start_host)
host_button.grid(row=3, column=1, columnspan=2, pady=5, sticky="ew")

status_label = ttk.Label(frame, text="Waiting")
status_label.grid(row=4, column=0, columnspan=3, pady=10)

# --- ★ここから追加/変更★ ---
# メッセージ入力欄
message_entry = ttk.Entry(frame, width=50, state="disabled")
message_entry.grid(row=5, column=0, columnspan=2, sticky="ew", padx=5, pady=5)
# メッセージ送信ボタン
send_button = ttk.Button(frame, text="Send", command=send_message, state="disabled")
send_button.grid(row=5, column=2, sticky="ew", padx=(5,0))
# Enterキーで送信するためのバインド
message_entry.bind("<Return>", send_message)

# Stopボタンの行番号を変更
stop_button = ttk.Button(frame, text="Stop", command=stop)
stop_button.grid(row=6, column=1, columnspan=2, pady=10, sticky="ew")
# --- ★ここまで追加/変更★ ---


# ウィンドウのリサイズに対応
root.columnconfigure(0, weight=1)
frame.columnconfigure(1, weight=1)

# キューの監視を開始
check_queue()

root.mainloop()