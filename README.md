# MHRise Save Editor(怪物猎人崛起 Steam 版存档编辑器)

一个仅面向 Steam 版《怪物猎人崛起:曙光》的存档查看、编辑与转移工具,操作 `win64_save` 目录下的三个角色存档(`data001Slot.bin` ~ `data003Slot.bin`)。

> [!WARNING]
> 编辑存档前请务必先备份(工具内「备份还原」页一键完成,CLI 用 `backup` 命令),并保持游戏关闭。所有字段映射均在真实存档上核验过(金钱、点数、道具箱、装备、等级、组合),但极端修改仍可能出现意外——请先在丢弃用槽位上试验。

## 工作原理

### 存档容器:DSSS v2 + Citrus 加密
- 文件头为 `DSSS` v2 容器,Steam 版 flags 含 CITRUS(0x04)标记;
- Citrus 体系:**SteamID64 派生 ECC 密钥 → AES-128-CBC 解密数据块**,每块带 SHA3-256 哈希,块大小 0x40000,尾部 0x1000 块(可为零);
- 曲线索引从 128 条预置曲线中按"解密后块哈希校验"暴力探测,因此**只需提供 SteamID64**(17 位数字 = 76561197960265728 + 32 位账号 ID;账号 ID 即 `userdata\<账号ID>\` 目录名,GUI 会自动推算);
- 外层校验和为声明长度前缀的 MurmurHash3(seed 0xffffffff)。
- 容器与加解密实现移植自 MIT 协议的 [jinghaihan/mhrise-save-converter](https://github.com/jinghaihan/mhrise-save-converter)(其研究基于 [kvasszn/ree-save-editor](https://github.com/kvasszn/ree-save-editor)),本仓库其余部分为原创。

### 类流:RSZ 结构与字段哈希
解密后是 RE 引擎的 RSZ 类流(`native_hash` + `class{hash, fields[]}`)。**本项目的关键发现:字段/类哈希 = murmur3_32(字段名, seed 0xffffffff)**,与社区 RSZ 字典(`assets/mhrise/rszmhrise.json`,25 万条)对照覆盖率 99.98%。所有编辑都以"类哈希 + 字段哈希"双重约束定位,防止同名误伤。

已核验的主要结构(类 → 关键字段):

| 类 | 哈希 | 内容 |
|---|---|---|
| `snow.data.HandMoney` | `079713ac` | `_Value` 当前金钱、`_TotalAddedValue` 累计 |
| `snow.data.VillagePoint` | `306735ee` | `_Point` 隐藏点/怪异点 |
| `snow.data.ItemBox` | `67696161` | `_InventoryList`:1800 × (ItemId, Num),空槽 = `0x04000000` |
| `snow.data.EquipBox` | `f0509899` | `_WeaponArmorInventoryList`:5000 × EquipmentInventoryData(武器/防具/护石) |
| `snow.data.EquipDataManager.SaveData1` | `960baed3` | `_PlEquipPack` 当前穿着、`_PlEquipMySetList`(装备组合×224)、`_HunterMySetList`(猎人组合×224)、`_PlArmorColorMySetData` |
| `snow.equip.PlEquipMySetData` | `d897238a` | 装备组合:`_Name`、`_IsUsing`、`_InventoryIndexList`(装备箱索引表) |
| `snow.data.HunterMySetData` | `af140835` | 猎人组合:`_Name` + `_ItemMysetIndex`/`_EquipMysetIndex`/`_OverwearMysetIndex` |
| `snow.data.ItemMySet` | `8923cd68` | 道具袋组合 ×40 |
| `snow.progress.ProgressSaveData` | `948b7b9d` | `_HunterRank/_MasterRank/_MysteryResearchLevel` + 各自 Point(等级权威数据) |
| `snow.data.GuildCardData` | `44541321` | 自名片:猎人名、等级显示副本(HunterRank/`_MasterRank`/LaboLv)、解禁布尔(isMRRelease 等);好友名片在数组中,绝不触碰 |

中文道具名(`assets/mhrise/item_names_cn.json`,1771 条)来自社区 16.0.0 道具表,与游戏内部枚举(`item_names.json`)按数值 ID 对齐。

## 主要功能

### GUI(`mhrise-save-editor-gui`)
- **自动发现存档**:读注册表 + Steam 库配置扫描所有 `userdata\<账号>\1446780\remote\win64_save`,快速选择自动填 SteamID64;
- **一般信息**:猎人名、等级(HR/MR/怪异研究)、金钱点数、容器信息;
- **玩家数据**:金钱、累计金钱、点数、三项等级编辑(应用后内存生效,保存时写盘);
- **道具箱编辑**:1800 槽中文道具名(可按名称/类别/ID 过滤)、单件数量修改、清空与新增;
- **存档转移**(同 Steam 账号的槽位间,均内存生效、统一写盘):
  - 道具箱按类别合并(消耗品/素材/弹药·瓶/换金·古董/其它),未勾选类别保留目标原样;
  - 装备转移按组件勾选(装备箱 / 穿着与装备组合登记 / 道具袋组合);
  - 等级进度转移(三项等级 + 点数);
  - 等级进度转移(三项等级 + 点数);
- **装备组合**:显示 224 槽装备组合(名称/占用),从来源存档按组合转移(可指定目标槽位或追加到第一个空位,可同时搬运组合引用的装备箱道具,同槽位覆盖),也可按装备箱槽位号单件转移;
- **进度与解禁**:等级点数编辑、名片解禁标志(显示副本)读写,并说明解禁钳制机制;
- **备份还原**:一键整目录备份到 `文档\MHR-Save-Backups\<账号>\<时间戳>`(逐字节校验、避开云同步范围),双击确认还原并自动重载;
- **写入原存档 (先自动备份)**:写盘前自动整目录备份,免去手动替换 `.edited.bin`;也可选「保存为新文件」手动替换;
- 后台线程执行所有 IO,UI 不卡顿;所有写盘操作都有备份兜底。

### CLI(`mhrise-save-editor`)
`inspect`(容器信息+等级)、`dump-json` / `apply-json`(无损 JSON 导出/回写)、`repack` / `roundtrip`(往返校验)、`backup` / `restore`、`diff`(两档结构差异)、`set-money` / `set-points`、`transfer-items` / `transfer-equipment`。

## 已知限制
- **等级解禁钳制**:未完成解禁任务链的存档,读档后游戏会把等级压回当前上限(会弹"获得成就"提示)。解禁状态在任务清通标志位图中(游戏特有打包结构,任务→位的映射需要游戏数据表),本工具暂不自动改写;建议游戏内解禁或用转移搬运进度。
- **任务标志**:同上,位图映射未逆向,不做自动修改。
- **中文输入**:winit 窗口库与 TSF 模式输入法的已知兼容问题;微软拼音开启「使用以前版本」兼容模式即可(详见帮助页)。
- 装备具体名称(武器/防具名)不在存档内,需要游戏数据表才能显示,当前以 ID 展示。

## 开发
```bash
cargo test          # 54 个单元/集成测试
cargo run --release --bin mhrise-save-editor-gui
scripts/package.py  # 打包 release 双 exe + README + LICENSE 到 dist/
```

代码结构:`src/container.rs`(DSSS/Citrus 容器)、`src/crypto/`(AES/ECC/SHA3)、`src/payload.rs`(RSZ 类流解析)、`src/edit.rs`(字段定位与编辑,含全部已核验哈希常量)、`src/items.rs`(道具名表)、`src/archive.rs`(备份)、`src/discover.rs`(存档发现)、`src/gui.rs`(eframe 界面)、`src/main.rs`(CLI)。Rust 2024 edition,`unsafe_code = forbid`。
