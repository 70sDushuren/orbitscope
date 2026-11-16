# testdata

离线测试样本。**全部为合成数据或公开示例格式**,不含从 Space-Track 下载的原始数据(用户协议限制再分发)。

- `sample.3le`:3 个合规校验和的 3LE(ISS 形态 + 两个合成物体,编号 9xxxx)
- `sample_with_errors.3le`:同上 + 一条校验和错误的记录
- `celestrak_gp.json`:CelesTrak 风格 OMM JSON(数值为 number),含 9 位编号与缺字段记录
- `spacetrack_gp.json`:Space-Track 风格 GP JSON(数值为字符串),含 RCS_SIZE、OBJECT_TYPE、已衰减物体
