# Migration Rollout

> 破壊的変更を文書/契約→新型→DB migration→runner→API/SDK→stage→computer→presence→self-evolution→旧path削除の順で直列化。

## 禁止
- 旧 job runtime と新 Task Runtime の並存
- target なし app allow rule の移行
- temporary flag / compat shim の残置

## 保証
- stage/CLI/API が同じ task 用語
- migration 前 backup から復元
- running 旧 job は Interrupted として報告

