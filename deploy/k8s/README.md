# Kubernetes Deployment (Kustomize + ArgoCD)

Sanitized reference manifests for deploying `openpilot-backend`, [`openpilot-frontend`](https://github.com/martinl/openpilot-frontend), `minikeyvalue`, and PostgreSQL 18 on Kubernetes.

## Prerequisites

- **CloudNativePG** (`postgresql.cnpg.io/v1`) for `postgres-cnpg.yaml` (creates the `openpilot-db-app` Secret used by `DATABASE_URL`)
- **Gateway API** (`gateway.networking.k8s.io/v1`) for `httproutes.yaml` (or replace with standard `Ingress` resources)
- *(Optional)* **Prometheus Operator** & **Grafana** (`kube-prometheus-stack`) — `backend-servicemonitor.yaml`, CNPG `enablePodMonitor`, and the `openpilot-grafana-dashboards` ConfigMap (`dashboards/*.json`) are provisioned automatically
- *(Optional)* **External Secrets Operator** (`external-secrets.io/v1`) if using `externalsecret.example.yaml`

## Quick Start

1. **Create the namespace and `openpilot-secrets` Secret** (or customize `externalsecret.example.yaml`):
   ```bash
   kubectl create namespace openpilot
   kubectl create secret generic openpilot-secrets -n openpilot \
     --from-literal=SERVER_SECRET="$(openssl rand -hex 32)" \
     --from-literal=GITHUB_CLIENT="<github-oauth-client-id>" \
     --from-literal=GITHUB_SECRET="<github-oauth-client-secret>" \
     --from-literal=ADMIN_JWT="<admin-jwt>" \
     --from-literal=MAPBOX_TOKEN="<mapbox-token>"
   ```
2. **Customize domains & Gateway** in `configmap.yaml` and `httproutes.yaml` (replace `example.com` and `main-gateway` in `kube-system`, or override them in a Kustomize overlay).
3. **Apply with Kustomize**:
   ```bash
   kubectl apply -k deploy/k8s/
   ```

## GitOps Remote Base (ArgoCD / Kustomize Overlay)

Rather than copying these base files into your private GitOps repository, reference `deploy/k8s` directly as a Kustomize remote base and keep only your environment-specific patches in your GitOps repo:

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization

namespace: openpilot

resources:
  # submodules=false skips cloning the large openpilot C++/Python submodule
  - https://github.com/martinl/openpilot-backend//deploy/k8s?ref=v0.1.0&submodules=false
  - externalsecret.yaml

images:
  - name: ghcr.io/martinl/openpilot-backend/backend
    newTag: v0.1.0
  - name: ghcr.io/martinl/openpilot-frontend
    newTag: v0.1.0
  - name: ghcr.io/martinl/openpilot-backend/minikeyvalue
    newTag: v0.1.0

patches:
  - path: configmap-patch.yaml
  - path: httproutes-patch.yaml
  - path: resources-patch.yaml
```

See `argocd-application.example.yaml` for an example ArgoCD `Application` manifest.
