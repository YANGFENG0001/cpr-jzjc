# 第三方组件

本插件同时提供 ModelTrace 与 Meow 两种行为指纹检测方法。Meow 检测器来自 https://github.com/R-lmmortal/codex-model-health-plugin 集成的 Meow LLM Detector（chen-006，PolyForm Noncommercial 1.0.0），源码固定在 `vendor/meow`，原许可证见同目录 `LICENSE`；使用、再分发或商业用途须遵守该许可证。

- ModelTrace： https://github.com/xqy2006/ModelTrace ，MIT。固定提交和数据来源见评分运行时内 `vendor/modeltrace/SOURCE.json`，原许可证见同目录 `LICENSE`。
- Codex Proxy RS SDK： https://github.com/zyycn/codex-proxy-rs ，Apache-2.0。固定提交见 `backend/Cargo.toml`。
- Meow LLM Detector： https://github.com/chen-006/meow-llm-detector ，PolyForm Noncommercial 1.0.0；经 codex-model-health-plugin 集成，基线版本与内容哈希见运行时内 `vendor/meow/gpt56_vnext/baselines`。
- @codex-proxy/ui： https://github.com/zyycn/codex-proxy-ui ，使用 0.6.0 发布包。
- Vue： https://github.com/vuejs/core ，MIT。
- Lucide： https://github.com/lucide-icons/lucide ，ISC。
- CPython、NumPy 及运行时原生库：原始许可证与声明汇总在 `THIRD_PARTY.txt`；运行时同时保留 Python 与 NumPy 原许可证文件。
- UI 构建配置参考 rs-reauth-manager，MIT，Copyright (c) 2026 Lemon-57。原许可证附于下文。

Meow 检测流程与题目载荷参考 https://github.com/R-lmmortal/codex-model-health-plugin ，检测器原源码保留在 `vendor/meow` 并遵守其非商业许可。

MIT License

Copyright (c) 2026 Lemon-57

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
