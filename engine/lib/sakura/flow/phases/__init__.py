"""Phase handlers; importing the package registers them with the engine."""

from sakura.flow import engine
from sakura.flow.phases import build, cleanup, contract, evaluate, intake, plan, pull_request, report

engine.register("intake", intake.run, intake.inputs_hash)
engine.register("contract", contract.run, contract.inputs_hash)
engine.register("plan", plan.run, plan.inputs_hash)
engine.register("build", build.run, build.inputs_hash)
engine.register("evaluate", evaluate.run, evaluate.inputs_hash)
engine.register("pr", pull_request.run)
engine.register("report", report.run, report.inputs_hash)
engine.register("cleanup", cleanup.run)
