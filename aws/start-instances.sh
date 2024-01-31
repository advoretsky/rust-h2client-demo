#!/bin/bash
aws ec2 start-instances --instance-ids $@ --output text --query 'StartingInstances[0].CurrentState.Name' | cat
