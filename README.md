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
    cargo run <source_cpp> <include_path> <user_list> <author> [options]
    ```
    例:
    ```bash
    cargo run example.cpp /usr/include "user1,user2" "author1" --clip
    ```

## オプション

- `--clip`: バンドルした結果をクリップボードにコピーします．
- `--write`: バンドルした結果を指定したファイルに出力します．
