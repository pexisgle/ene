# Home Assistant ツールガイド

`ene-plugin-homeassistant` は [Home Assistant REST API](https://developers.home-assistant.io/docs/api/rest/)
経由でスマートホームを操作します: エンティティ状態の取得、スイッチ/照明/
スマートプラグの制御、空調の温度設定です。組み込みツールプラグインであり、
新規インストール時に自動的に起動します。

## 設定

このプラグインは自身の Home Assistant インスタンスと通信するため、
アクションが動作する前にベース URL と長期アクセストークンが必要です:

- `base_url` — インスタンスの URL。例: `http://homeassistant.local:8123`。
  リバースプロキシのパスプレフィックスも利用でき、末尾は `/` で終わる
  必要があります (例: `https://home.example.com/ha/`)。
- `token` — Home Assistant の **プロフィール → セキュリティ → 長期アクセス
  トークン** で作成する長期アクセストークン。

`settings.json` の `plugins.list.homeassistant.config` に設定します:

```json
{
  "plugins": {
    "list": {
      "homeassistant": {
        "enable": true,
        "config": {
          "base_url": "http://homeassistant.local:8123",
          "token": "your-long-lived-token"
        }
      }
    }
  }
}
```

トークン欄はプラグインの設定スキーマで `x-ene-secret` に指定されているため、
Ene は UI 上でマスクし、ログからは編集 (redact) します。環境変数
`ENE_PLUGINS__LIST__HOMEASSISTANT__CONFIG__TOKEN` でも同じ値を設定できます。

プラグインは `x-ene-credentials` で資格情報 ID `homeassistant` (プライベート、
種別 `api_key`) も宣言しており、Ene の資格情報ヘルパーがホスト側の
クレデンシャルクライアント API をメインラインに取り込んだ際に、保存済み
トークンを配信できる準備ができています。それまでは上記の通りプラグイン
設定からトークンを読み取ります。

## アクション

### `homeassistant.state`

エンティティの現在状態・属性・最終更新時刻を読み取ります:

```json
{"entity_id": "light.living_room"}
```

```json
{"entity_id": "sensor.outdoor_temperature"}
```

このアクションは読み取り専用で、承認プロンプトなしで実行されます。
ただし自宅の Home Assistant インスタンスへのネットワーク通信は発生します。

### `homeassistant.turn_on` / `homeassistant.turn_off`

スイッチ・照明・スマートプラグなどのオン/オフ機器を操作します:

```json
{"entity_id": "switch.kitchen_plug"}
```

これらのアクションは機器の物理状態を変更するため、Home Assistant への
リクエスト前に明示的なユーザー承認が必要です。承認プロンプトには対象の
エンティティ ID と操作内容が表示されます。承認は現在のターンのみ有効で、
「このセッションでは許可」を選ぶと会話の間中許可が続きます。

### `homeassistant.set_temperature`

空調・ヒーター・サーモスタットなどの climate エンティティの目標温度を
設定します:

```json
{"entity_id": "climate.living_room", "temperature": 22.0}
```

任意で HVAC モードを同時に指定できます:

```json
{"entity_id": "climate.living_room", "temperature": 18.0, "hvac_mode": "heat"}
```

指定できるモード: `off`・`heat`・`cool`・`heat_cool`・`auto`・`dry`・
`fan_only`。オン/オフ操作と同様に物理状態を変更するため、明示的な
ユーザー承認が必要です。

## トラブルシューティング

- エンティティ ID は小文字英数字とアンダースコアのみの `domain.entity`
  形式です — `light.living_room` は有効ですが `Living Room` は無効です。
- リクエストは 10 秒でタイムアウトします。Home Assistant が遅いネットワーク
  やリバースプロキシの背後にある場合は、まず base URL を確認してください。
- Home Assistant のエラーレスポンス (`{"code", "message"}`) はツールの
  エラーにそのまま渡されるため、拒否されたサービス呼び出しは理由が
  わかります。
- トークンがツールの結果やエラーメッセージに含まれることはありません。
  HTTP 401 が返る場合は、トークンの未設定・失効・取り消しを確認して
  ください。
