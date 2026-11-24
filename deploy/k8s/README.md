# Kubernetes Deployment (Kustomize + ArgoCD)

Sanitized reference manifests for deploying `openpilot-backend`, [`openpilot-frontend`](https://github.com/martinl/openpilot-frontend), `minikeyvalue`, and PostgreSQL 18 on Kubernetes.

## Prerequisites

- **CloudNativePG** (`postgresql.cnpg.io/v1`) for `postgres-cnpg.yaml` (creates the `openpilot-db-app` Secret used by `DATABASE_URL`)
- **Gateway API** (`gateway.networking.k8s.io/v1`) for `httproutes.yaml` (or replace with standard `Ingress` resources)
- *(Optional)* **Prometheus Operator** (`monitoring.coreos.com/v1`) for `backend-servicemonitor.yaml` and CNPG `enablePodMonitor`
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
   - *Note:* If your `openpilot-frontend` image mutates `/usr/share/nginx/html` at container startup via `sed`, set `readOnlyRootFilesystem: false` on `openpilot-frontend` (or mount an `emptyDir` for runtime config).
3. **Apply with Kustomize**:
   ```bash
   kubectl apply -k deploy/k8s/
   ```
4. *(Optional)* **Import Grafana dashboards**:
   ```bash
   kubectl create configmap openpilot-grafana-dashboards -n openpilot \
     --from-file=deploy/grafana/dashboards/openpilot-red.json \
     --from-file=deploy/grafana/dashboards/openpilot-app.json
   kubectl label configmap openpilot-grafana-dashboards -n openpilot grafana_dashboard=1
   kubectl annotate configmap openpilot-grafana-dashboards -n openpilot grafana_folder=Openpilot
   ```

## GitOps (ArgoCD)

Use `argocd-application.example.yaml` as a template pointing to your own GitOps repository overlay (where you pin image tags, domain overrides, and `storageClassName` if not using the cluster default).
