# ⚡ unisoc-tool-rs

**Cross-Platform (Linux, Windows, macOS, Android/ARM64) Service, Flashing & Diagnostic Tool for Unisoc / Spreadtrum Devices**  
*Кроссплатформенный (Linux, Windows, macOS, Android/ARM64) сервисный инструмент для прошивки, диагностики и обслуживания смартфонов на базе процессоров Unisoc / Spreadtrum.*

[![Rust](https://img.shields.io/badge/Language-Rust_2021-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Linux_%7C_Windows_%7C_macOS_%7C_Android-blue.svg)](#)
[![Arch](https://img.shields.io/badge/Architecture-x86__64_%7C_arm64_%7C_armv7-purple.svg)](#)
[![CI/CD](https://github.com/Milordick/unisoc-tool-rs/actions/workflows/release.yml/badge.svg)](https://github.com/Milordick/unisoc-tool-rs/actions)
[![Status](https://img.shields.io/badge/Status-Beta_%2F_Experimental-red.svg)](#)
[![License](https://img.shields.io/badge/License-GPL--3.0-green.svg)](#-лицензия--license)

🌍 **[Русская документация](#-описание-проекта-ru)** | **[English Documentation](#-project-overview-en)**

---

> [!CAUTION]
> ### ⚠️ ВНИМАНИЕ: СТАТУС БЕТА-ВЕРСИИ И ОТКАЗ ОТ ОТВЕТСТВЕННОСТИ (DISCLAIMER)
> **Программа находится в стадии активной разработки (BETA) и НЕ является полностью отлаженным коммерческим продуктом.**  
> В коде могут присутствовать ошибки, непредвиденные сбои и несовместимости с отдельными ревизиями процессоров, таблицами разметки и версиями загрузчиков.
> 
> Низкоуровневая работа с BootROM, отправка кастомного кода в оперативную память (FDL1/FDL2), стирание и прямая запись разделов eMMC/UFS несут **прямой риск превращения устройства в состояние «кирпича» (Hard Brick), повреждения калибровок радиомодуля (IMEI/NVRAM) и безвозвратной потери данных.**
> 
> - **ВСЕГДА** создавайте резервную копию калибровок (`Backup NVRAM`) перед любыми действиями!
> - **ВСЕ ОПЕРАЦИИ ВЫ ВЫПОЛНЯЕТЕ ИСКЛЮЧИТЕЛЬНО НА СВОЙ СТРАХ И РИСК.** Автор и контрибьюторы не несут никакой ответственности за окирпиченные телефоны, сгоревшие платы, потерянные данные или иной ущерб.

---

## 🇷🇺 Описание проекта (RU)

**`unisoc-tool-rs`** — это высокопроизводительный инструмент на языке **Rust** с интерактивным терминальным интерфейсом (**TUI**) и режимом командной строки (**CLI**) для низкоуровневой работы с устройствами **Unisoc (Spreadtrum)** в режимах **BootROM (`SPRD3`)**, **FDL1** и **FDL2**.

Программа создана как открытая, чистая и независимая альтернатива закрытым сервисным утилитам (таким как *MRST Unisoc Tool*). Она работает автономно, без онлайн-авторизаций, донглов, привязок к железу (HWID) и протектора (VMProtect).

---

### 🖥️ Поддерживаемые архитектуры и операционные системы

Инструмент компилируется и работает практически на любом современном процессоре и ОС:

| ОС | Архитектура | Статус | Примечания |
| :--- | :--- | :---: | :--- |
| **Linux** | `x86_64` |  Полная поддержка | Проверено на Ubuntu, Debian, Arch, Fedora. Нативный USB (`nusb`) и `spd_dump`. |
| **Linux** | `ARM64 (aarch64)` |  Полная поддержка | Raspberry Pi 4/5, Orange Pi, ноутбуки на ARM. Собирается нативно: `cargo build`. |
| **Windows** | `x86_64` |  Полная поддержка | Windows 10/11. Поддержка `spd_dump.exe`, `Channel9.dll`, драйверов `SPRD U2S Diag`. |
| **Windows** | `ARM64` |  Работает | Windows 11 on ARM (Snapdragon X Elite / Surface) через встроенную x86-эмуляцию для `spd_dump.exe`. |
| **macOS** | `Apple Silicon (M1–M4)` / `Intel` |  Поддерживается (Beta) | Работает через IOKit без root. Требуется компиляция `spd_dump` под macOS и USB 2.0 хаб. |
| **Android** | `ARM64 (Termux)` |  Поддерживается (Root) | Возможность прошивать один телефон с другого через USB-OTG переходник! |

#### Особенности запуска на macOS:
1. **Сборка `spd_dump` под macOS**: бинарники в `loaders/` скомпилированы под Linux/Windows. На Mac необходимо скомпилировать Mach-O бинарник `spd_dump` через Homebrew:
   ```bash
   brew install libusb
   git clone https://github.com/tomking062/spreadtrum_flash.git
   cd spreadtrum_flash
   clang -O2 spd_dump.c -I$(brew --prefix libusb)/include/libusb-1.0 -L$(brew --prefix libusb)/lib -lusb-1.0 -o spd_dump
   cp spd_dump /путь/к/unisoc-tool-rs/loaders/
   ```
2. **Аппаратный нюанс контроллеров Apple Silicon Type-C**: порты Thunderbolt/Type-C на Mac с процессорами M1/M2/M3/M4 могут давать сбой по таймауту (`LIBUSB_ERROR_TIMEOUT`) при сбросе шины во время перезапуска телефона из BootROM в FDL1. **Решение:** подключайте телефон через обычный переходник или хаб **USB-C → USB-A (USB 2.0)**.

---

### ✨ Основные возможности

1. **Двойной транспортный движок (Dual Engine)**:
   - **Интеграция с `spd_dump` / `spd_dump.exe`**: полная поддержка обхода проверки подписи RSA в BROM (`exec_addr 0x65015f08` / `custom_exec_no_verify_65015f08.bin`, CVE-2022-38694), хендшейка `Da_Info (0x96)` и работы с разблокированными загрузчиками `fdl1-dl.bin` / `fdl2-dl.bin` (без ошибки `0x00fe` от SLA-блокировки Transsion).
   - **Нативный Rust-движок (`nusb` + `serialport`)**: прямое подключение к USB Bulk-эндпоинтам (`VID 0x1782`, `PID 0x4d00`) без привязки ядерного драйвера на Linux/macOS или работа через виртуальный COM-порт (`SPRD U2S Diag (COMx)` на Windows / `/dev/ttyUSB*` на Linux).
2. **Интерактивный TUI-интерфейс**:
   - Живой лог со сворачиванием спама строк, статус-бар и индикаторы выполнения.
   - Автоматический выбор Unisoc-устройств с ручным селектором портов по клавише `p`.
   - Защита от случайного выполнения деструктивных операций (подтверждение клавишей `y`) и отмена ожидания телефона по `Esc` / `Ctrl+C`.
3. **Аппаратная диагностика и проверка загрузчика**:
   - Чтение версии BootROM (`SPRD3`), `Chip UID` (`BSL_REP_READ_CHIP_UID`) и автоматическая проверка реального статуса **Bootloader Unlock** по токену в разделе `miscdata` (смещение `8192` / `0x2000`).
4. **Сервисные операции и работа с разделами**:
   - **Read Live Partitions**: чтение разметки GPT напрямую из устройства (`partition_live.xml`).
   - **Backup / Restore NVRAM**: резервное копирование и восстановление разделов калибровок и IMEI (`prodnv`, `miscdata`, `l_fixnv1_a`, `l_fixnv1_b`).
   - **Dump / Flash Partition**: дамп `boot_b` (`64 МБ`) для патчинга Magisk / KernelSU и запись кастомного ядра `boot.img`.
   - **Reset FRP & Factory Reset**: сброс Google FRP (`persist`, `misc`) и форматирование пользовательских данных (`userdata`, `metadata`, `cache`).
5. **Работа с заводскими прошивками `.pac`**:
   - Просмотр метаданных (`pac-info`), распаковка всех образов (`unpack-pac`) и прямая прошивка (`flash-pac`) с поддержкой выбора контроллера памяти (`auto`, `ufs`, `emmc`).

---

### 📱 Поддерживаемые процессоры и устройства

Встроенная база (`src/devices.rs`) включает свыше 48 моделей смартфонов:

| Семейство SoC | Адрес FDL1 | Адрес FDL2 | Популярные смартфоны |
| :--- | :--- | :--- | :--- |
| **Tiger T606 / T612 / T616 / T618** (`UMS9230`) | `0x65000800` | `0x9EFFFE00` | **Infinix** Hot 30i (X669/X669D), Hot 12 Play, Smart 7/8; **Tecno** Spark 8C/10C, Pova Neo 2; **Realme** C30, C31, C33, C35, C51, C53, Narzo 50A Prime |
| **Tiger T610 / T700** (`UMS512`) | `0x00005500` / `0x65000800` | `0x9EFFFE00` | **Realme** C21Y, C25Y, Pad; **Micromax** IN 2b; **Nokia** G20, G21, T20; **ZTE** Axon 20 |
| **Spreadtrum SC9863A** | `0x65000800` / `0x00005500` | `0x9EFFFE00` | **Realme** C11 (2021), C12, Narzo 50i; **Infinix** Smart 5/6; **Tecno** Pop 5/6; **ZTE** Blade A51, A71, A5 2020 |
| **Spreadtrum SC9832E / SC7731E** | `0x00005500` | `0x80000000` | Бюджетные устройства Itel, ZTE Blade A3, BQ, DEXP |

---

### ⚡ Быстрая установка в 1 команду (Recommended)

Умный скрипт автоматически определит вашу операционную систему (`Linux`, `macOS`, `Android/Termux`, `Windows`), архитектуру процессора (`x86_64`, `arm64/aarch64`), скачает последнюю скомпилированную версию с GitHub Actions, установит необходимые файлы загрузчиков `loaders/` и добавит `unisoc` в PATH.

#### 🐧 Linux / 🍏 macOS / 📱 Android (Termux):
```bash
curl -fsSL https://raw.githubusercontent.com/Milordick/unisoc-tool-rs/main/install.sh | bash
```

#### 🪟 Windows (PowerShell):
```powershell
irm https://raw.githubusercontent.com/Milordick/unisoc-tool-rs/main/install.ps1 | iex
```

---

### 🔄 Встроенное самообновление (Self-Updater)

В программу встроен механизм автоматического обновления. Для проверки и установки последней версии с GitHub достаточно выполнить:
```bash
unisoc update
```
*(или `unisoc update --force` для принудительной переустановки)*

---

### 🛠️ Ручная сборка из исходников

```bash
# Клонирование репозитория
git clone https://github.com/Milordick/unisoc-tool-rs.git
cd unisoc-tool-rs

# Сборка оптимизированного бинарника
cargo build --release

# Запуск интерактивного TUI
./target/release/unisoc          # Linux / macOS
.\target\release\unisoc.exe      # Windows
```

#### Настройка udev (Linux):
```bash
sudo ./target/release/unisoc setup-linux
```

#### Структура папки `loaders/`:
```text
loaders/
├── spd_dump                           # Linux x86_64 / arm64 движок
├── spd_dump.exe                       # Windows движок
├── Channel9.dll                       # Windows-библиотека протокола
├── Channel.ini                        # Конфигурация для Windows
├── custom_exec_no_verify_65015f08.bin # Эксплойт BROM RSA bypass (CVE-2022-38694)
├── fdl1-dl.bin                        # Патченный FDL1 (без SLA)
└── fdl2-dl.bin                        # Патченный FDL2 (без ошибки 0x00fe)
```

---
---

## 🇬🇧 Project Overview (EN)

**`unisoc-tool-rs`** is an open-source, memory-safe, cross-platform service, flashing, and diagnostics suite written in **Rust** for **Unisoc / Spreadtrum** SoC devices. It supports Linux, Windows, macOS, and Android (ARM64/Termux), featuring both an interactive **TUI** and a scriptable **CLI**.

> [!CAUTION]
> ### ⚠️ EXPERIMENTAL / BETA SOFTWARE NOTICE
> This tool interacts directly with BootROM, loads execution payloads into device RAM, and performs raw partition reads/writes on eMMC and UFS storage.
> Bugs, unhandled edge cases, or incompatible vendor FDLs **can result in hard-bricked devices or lost NVRAM/IMEI calibration data.**
> **Always make a full NVRAM backup first.** Use strictly at your own risk.

---

### 💻 Architecture & OS Compatibility Matrix

- **Linux (`x86_64`, `ARM64`)**: Native zero-driver bulk USB access via `nusb` + full `spd_dump` integration.
- **Windows (`x86_64`, `ARM64`)**: Full support for Spreadtrum `SPRD U2S Diag` COM drivers, `spd_dump.exe`, `Channel9.dll`, and `Channel.ini`.
- **macOS (`Apple Silicon M1–M4`, `Intel`)**: Native `nusb` works without root via IOKit. Requires building `spd_dump` via Homebrew (`brew install libusb`). Connect phones via an external **USB-C to USB-A (USB 2.0) Hub** to avoid Apple Silicon USB-C re-enumeration resets.
- **Android / Termux (`ARM64`, Root)**: Can be compiled with `cargo` inside Termux to flash phones using USB OTG.

---

### ⚡ One-Line Quick Install

#### Linux / macOS / Android (Termux):
```bash
curl -fsSL https://raw.githubusercontent.com/Milordick/unisoc-tool-rs/main/install.sh | bash
```

#### Windows (PowerShell):
```powershell
irm https://raw.githubusercontent.com/Milordick/unisoc-tool-rs/main/install.ps1 | iex
```

#### Self-Update:
```bash
unisoc update
```

---

## 🤝 Благодарности и используемые наработки (Credits & Attribution)

Проект создан на основе исследований безопасности, открытых наработок сообщества и результатов обратной разработки:

1. **[tomking062 / spreadtrum_flash (`spd_dump`)](https://github.com/tomking062/spreadtrum_flash)**:
   - Базовая реализация протокола Spreadtrum BSL/HDLC framing (`0x7E`), команд FDL1/FDL2, BROM handshake и алгоритмов обмена данными по USB Bulk.
2. **[bkerler / loadraw](https://github.com/bkerler/loadraw) & Unisoc Security Researchers**:
   - Исследование уязвимости **CVE-2022-38694** в BootROM процессоров Unisoc (UMS9230/T606, UMS512 и др.) и бинарный пейлоад обхода проверки RSA-подписи [`custom_exec_no_verify_65015f08.bin`](file:///var/mnt/storage/Загрузки/unisoc-tool-rs%20%281%29/unisoc-tool-rs/loaders/custom_exec_no_verify_65015f08.bin).
3. **MRST Unisoc Tool / Channel9 Research (REA / Ghidra Analysis)**:
   - Анализ бинарников `MRST_Loader_UniSoc.exe`, `spd_dump.exe` и `Channel9.dll` позволил выявить:
     - Особенности работы Windows-драйвера `SPRD U2S Diag` и динамической загрузки `Channel9.dll`;
     - Причину ошибки `0x00fe` (Transsion SLA lock в стоковых `fdl2.bin`) и необходимость использования разблокированных загрузчиков `fdl1-dl.bin` / `fdl2-dl.bin`;
     - Смещение токена разблокировки загрузчика в разделе `miscdata` (`0x2000` / `8192`).
4. **Экосистема Rust**:
   - [`nusb`](https://github.com/kevinmehall/nusb) — чистая асинхронная/блокирующая реализация USB bulk endpoints без libusb.
   - [`crossterm`](https://github.com/crossterm-rs/crossterm) — кроссплатформенный TUI-рендеринг в терминале.
   - [`serialport-rs`](https://github.com/gitlab-de/serialport-rs) — работа с последовательными портами ОС.
   - [`clap`](https://github.com/clap-rs/clap) — парсер аргументов командной строки.

---

## 📜 Лицензия / License

Этот проект распространяется под лицензией **GNU General Public License v3.0 (GPL-3.0)**.  
Исходный код открыт для изучения, аудита и модификаций.

```text
unisoc-tool-rs — Cross-Platform Unisoc Service Tool
Copyright (C) 2026 Contributors

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
GNU General Public License for more details.
```
