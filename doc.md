## aleo 挖矿程序使用
---
### CPU
#### 参数
带 * 号的参数必须设置
- address: aleo 钱包地址 *
- name: 矿工名 *
- pool-ip: 矿池 ip 地址，如 127.0.0.1:8080 *
- worker: 并行的计算任务的数量
- thread-per-worker: 每个计算任务要使用的线程数

note: 为了更高效，应尽量使 worker 乘 thread-per-worker 小于设备的总线程数

### 例子
```
aleo-miner-cpu mine-cpu --address aleo1m3x4er3nkjtxgtlyq9r3nl0vcyt3mmdgute6w42sm3vuwqvn0yxqct5nm3 --name miner1 --pool-ip 127.0.0.1:8080 --worker 2 --thread-per-worker 4
```

---

### GPU
#### 参数
带 * 号的参数必须设置
- address: aleo 钱包地址 *
- name: 矿工名 *
- pool-ip: 矿池 ip 地址，如 127.0.0.1:8080 *
- gpus: 要使用的 gpu，从 0 开始索引
- worker-per-gpu: 每个 gpu 上并行任务的数量
### 例子
```
aleo-miner-gpu mine-gpu --address aleo1m3x4er3nkjtxgtlyq9r3nl0vcyt3mmdgute6w42sm3vuwqvn0yxqct5nm3 --name miner2 --pool-ip 127.0.0.1:8080 --worker-per-gpu 2 --gpus 0 1 3
```
---