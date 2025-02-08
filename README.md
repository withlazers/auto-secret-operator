# auto-secret-operator

Dead simple secret generation for kubernetes.

## Installation

```bash
helm repo add withlazers https://charts.withlazers.dev
helm install auto-secret-operator withlazers/auto-secret-operator
```

### Example

Just create an empty secret with the annotation `auto-secret.k8s.eboland.de/gen: "PASSWORD: default"`:

```yaml
apiVersion: v1
kind: Secret
metadata:
  annotations:
    auto-secret.k8s.eboland.de/gen: |
      PASSWORD: default
  name: auto-secret
  namespace: default
type: Opaque
data:
  USERNAME: dXNlcg==
```

auto-secret-operator will generate a random password and store it in the secret:

```yaml
...
data:
  USERNAME: dXNlcg==
  PASSWORD: MzlkPjFfejJLMjw3NkZ3QWAieVZZQEdnfnt+Sj4rV1M=
...
```
