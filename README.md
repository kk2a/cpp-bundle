# cpp-bundle

`cpp-bundle`は，C++で書かれたプログラム群をまとめて一つのファイルにまとめるためのツールです．

## 特徴
- 複数のC++ファイルを一つのファイルにまとめる
- クリップボードへのコピー機能
- ファイルへの出力機能

## 使い方

1. リポジトリのクローン
    ```bash
    git clone https://github.com/kk2a/cpp-bundle.git
    cd cpp-bundle
    ```

2. ビルド
    ```bash
    cargo build
    ```

3. 実行
    ```bash
    cargo run <input_file> <include_path> <author> [options]
    ```
    例:
    ```bash
    cargo run example.cpp /usr/include "Your Name" --clip
    ```

## オプション

- `--clip`, `-c`: バンドルした結果をクリップボードにコピーします
- `--write`, `-w`: バンドルした結果を入力ファイルに上書き出力します
- `--no-format`: コードのフォーマット（空白の整理や改行の削除）を無効にします

## フォーマットの制御
コード内で特定の領域のフォーマットを制御したい場合は，以下の特殊コメントを使用できます：

```cpp
// BEGIN_PRESERVE_NEWLINES
#define COMPLEX_MACRO(x) \
    do { \
        something(); \
    } while(0)
// END_PRESERVE_NEWLINES
```

この範囲内では改行と空白が保持されます．マクロの定義などで使用してください．
